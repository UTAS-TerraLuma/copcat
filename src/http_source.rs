//! A [`ByteSource`] that reads COPC byte ranges over HTTP(S).
//!
//! `copc-streaming` ships sources for local files and memory only. Requests
//! here block, which is fine because loading already runs on its own thread
//! and drives the reader's futures with `pollster`.

use std::thread;

use copc_streaming::{ByteSource, CopcError};

/// Upper bound on concurrent range requests in one [`ByteSource::read_ranges`]
/// call. Enough to hide per-request latency without hammering object stores.
const MAX_PARALLEL_REQUESTS: usize = 8;

pub struct HttpSource {
    url: String,
    /// Shared so parallel requests reuse pooled connections.
    agent: ureq::Agent,
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error(transparent)]
    Request(#[from] ureq::Error),
    #[error(
        "server replied {status} instead of 206 Partial Content; \
         COPC over HTTP needs a server that supports range requests"
    )]
    RangeNotSupported { status: u16 },
    #[error("asked for {expected} bytes at offset {offset} but received {received}")]
    ShortRead {
        offset: u64,
        expected: u64,
        received: usize,
    },
}

impl From<HttpError> for CopcError {
    fn from(error: HttpError) -> Self {
        CopcError::ByteSource(Box::new(error))
    }
}

impl HttpSource {
    pub fn new(url: String) -> Self {
        Self {
            url,
            agent: ureq::Agent::new_with_defaults(),
        }
    }

    fn get_range(&self, offset: u64, length: u64) -> Result<Vec<u8>, HttpError> {
        if length == 0 {
            return Ok(Vec::new());
        }
        let last_byte = offset + length - 1;
        let mut response = self
            .agent
            .get(&self.url)
            .header("Range", format!("bytes={offset}-{last_byte}"))
            .call()?;
        // A server that ignores Range sends the whole file with 200, which for
        // a multi-gigabyte cloud means downloading all of it. Bail out instead.
        let status = response.status().as_u16();
        if status != 206 {
            return Err(HttpError::RangeNotSupported { status });
        }
        // ureq errors once the body *reaches* the limit, so allow one byte of
        // slack; the length check below still rejects anything oversized.
        let bytes = response
            .body_mut()
            .with_config()
            .limit(length + 1)
            .read_to_vec()?;
        if bytes.len() as u64 != length {
            return Err(HttpError::ShortRead {
                offset,
                expected: length,
                received: bytes.len(),
            });
        }
        Ok(bytes)
    }

    fn get_ranges(&self, ranges: &[(u64, u64)]) -> Result<Vec<Vec<u8>>, HttpError> {
        let per_thread = ranges.len().div_ceil(MAX_PARALLEL_REQUESTS).max(1);
        thread::scope(|scope| {
            let workers: Vec<_> = ranges
                .chunks(per_thread)
                .map(|batch| {
                    scope.spawn(move || {
                        batch
                            .iter()
                            .map(|&(offset, length)| self.get_range(offset, length))
                            .collect::<Result<Vec<_>, _>>()
                    })
                })
                .collect();
            // Joining in spawn order keeps results in the same order as `ranges`.
            let mut results = Vec::with_capacity(ranges.len());
            for worker in workers {
                let batch = worker.join().expect("range request thread panicked")?;
                results.extend(batch);
            }
            Ok(results)
        })
    }

    fn content_length(&self) -> Result<Option<u64>, HttpError> {
        let response = self.agent.head(&self.url).call()?;
        Ok(response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok()?.parse().ok()))
    }
}

impl ByteSource for HttpSource {
    async fn read_range(&self, offset: u64, length: u64) -> Result<Vec<u8>, CopcError> {
        Ok(self.get_range(offset, length)?)
    }

    async fn size(&self) -> Result<Option<u64>, CopcError> {
        Ok(self.content_length()?)
    }

    async fn read_ranges(&self, ranges: &[(u64, u64)]) -> Result<Vec<Vec<u8>>, CopcError> {
        Ok(self.get_ranges(ranges)?)
    }
}
