//! Formatting values for people to read, shared by the viewer and the
//! command line.

/// Short human-readable counts: `950`, `61.2 k`, `10.65 M`.
pub(crate) fn format_count(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{:.1} k", count as f64 / 1e3),
        _ => format!("{:.2} M", count as f64 / 1e6),
    }
}

/// The last segment of a URL or path, ignoring any query string.
pub(crate) fn file_name(source: &str) -> &str {
    let without_query = source.split(['?', '#']).next().unwrap_or(source);
    without_query
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_should_take_last_url_segment_without_query() {
        assert_eq!(
            file_name("https://example.com/a/b/site.copc.laz?sig=abc"),
            "site.copc.laz"
        );
    }

    #[test]
    fn file_name_should_handle_windows_paths() {
        assert_eq!(file_name(r"C:\data\site 1.copc.laz"), "site 1.copc.laz");
    }

    #[test]
    fn format_count_should_use_millions_for_large_counts() {
        assert_eq!(format_count(10_653_336), "10.65 M");
    }

    #[test]
    fn format_count_should_use_thousands_for_node_sized_counts() {
        assert_eq!(format_count(61_201), "61.2 k");
    }
}
