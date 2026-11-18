//! What kind of coordinate reference system a cloud is in, read from its
//! WKT or GeoTIFF keys. Only enough to name the axes: a projected CRS has
//! eastings and northings, so the gizmo can say E, N and U.

/// The LAS user id of the CRS records.
pub const PROJECTION_USER_ID: &str = "LASF_Projection";
/// The OGC WKT record, which COPC requires.
pub const WKT_RECORD_ID: u16 = 2112;
/// The GeoTIFF GeoKeyDirectoryTag record, used by older LAS files.
pub const GEOKEYS_RECORD_ID: u16 = 34735;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrsKind {
    /// Eastings and northings, such as UTM.
    Projected,
    /// Longitude and latitude, or earth-centred XYZ.
    Geographic,
    /// Something else, such as only a vertical CRS.
    Other,
}

/// The kind of CRS a WKT (version 1 or 2) describes, or `None` if it isn't
/// recognisable WKT. For a compound or bound CRS, the horizontal part decides.
pub fn kind_from_wkt(wkt: &str) -> Option<CrsKind> {
    // Wrappers whose first CRS is the one that matters: a compound CRS lists
    // its horizontal part first, and a bound CRS its source first.
    const WRAPPERS: [&str; 4] = ["COMPD_CS", "COMPOUNDCRS", "BOUNDCRS", "SOURCECRS"];
    let keyword = keywords(wkt).find(|keyword| !WRAPPERS.contains(&keyword.as_str()))?;
    Some(match keyword.as_str() {
        "PROJCS" | "PROJCRS" | "PROJECTEDCRS" => CrsKind::Projected,
        "GEOGCS" | "GEOGCRS" | "GEOGRAPHICCRS" | "GEODCRS" | "GEODETICCRS" | "GEOCCS" => {
            CrsKind::Geographic
        }
        _ => CrsKind::Other,
    })
}

/// The WKT keywords (the words before each `[` or `(`), in order, upper-cased
/// and skipping quoted text.
fn keywords(wkt: &str) -> impl Iterator<Item = String> + '_ {
    let mut in_quotes = false;
    let mut word = String::new();
    wkt.chars().filter_map(move |c| {
        if c == '"' {
            in_quotes = !in_quotes;
            word.clear();
            return None;
        }
        if in_quotes {
            return None;
        }
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c.to_ascii_uppercase());
            None
        } else if (c == '[' || c == '(') && !word.is_empty() {
            Some(std::mem::take(&mut word))
        } else {
            if !c.is_whitespace() {
                word.clear();
            }
            None
        }
    })
}

/// The kind of CRS a GeoTIFF GeoKeyDirectoryTag (as stored in LAS, little
/// endian `u16`s) describes, or `None` if it doesn't say.
pub fn kind_from_geokeys(data: &[u8]) -> Option<CrsKind> {
    const MODEL_TYPE: u16 = 1024;
    const GEOGRAPHIC_TYPE: u16 = 2048;
    const PROJECTED_TYPE: u16 = 3072;
    let shorts: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let count = usize::from(*shorts.get(3)?);
    // Each key is (id, location, count, value); a location of 0 means the
    // value is the short itself.
    let keys: Vec<(u16, u16)> = shorts
        .get(4..)?
        .as_chunks::<4>()
        .0
        .iter()
        .take(count)
        .map(|&[id, location, _, value]| (id, if location == 0 { value } else { 0 }))
        .collect();
    let value = |id: u16| keys.iter().find(|(key, _)| *key == id).map(|(_, v)| *v);
    match value(MODEL_TYPE) {
        Some(1) => Some(CrsKind::Projected),
        Some(2 | 3) => Some(CrsKind::Geographic),
        Some(_) => Some(CrsKind::Other),
        None if value(PROJECTED_TYPE).is_some() => Some(CrsKind::Projected),
        None if value(GEOGRAPHIC_TYPE).is_some() => Some(CrsKind::Geographic),
        None => None,
    }
}

/// The kind of CRS given a file's WKT and GeoTIFF keys, either of which may
/// be missing. WKT wins, as COPC requires it.
pub fn crs_kind(wkt: Option<&[u8]>, geokeys: Option<&[u8]>) -> Option<CrsKind> {
    let from_wkt = wkt.and_then(|bytes| {
        let text = String::from_utf8_lossy(bytes);
        kind_from_wkt(text.trim_end_matches('\0'))
    });
    from_wkt.or_else(|| geokeys.and_then(kind_from_geokeys))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTM_WKT1: &str = r#"PROJCS["WGS 84 / UTM zone 55S",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],UNIT["metre",1],AUTHORITY["EPSG","32755"]]"#;
    const UTM_WKT2: &str = r#"PROJCRS["WGS 84 / UTM zone 55S",
        BASEGEOGCRS["WGS 84",DATUM["World Geodetic System 1984",ELLIPSOID["WGS 84",6378137,298.257223563]]],
        CONVERSION["UTM zone 55S",METHOD["Transverse Mercator"]],
        CS[Cartesian,2],AXIS["(E)",east],AXIS["(N)",north],
        ID["EPSG",32755]]"#;
    const GEOGRAPHIC_WKT1: &str = r#"GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433]]"#;
    const GEOGRAPHIC_WKT2: &str = r#"GEOGCRS["WGS 84",DATUM["World Geodetic System 1984",ELLIPSOID["WGS 84",6378137,298.257223563]],CS[ellipsoidal,2]]"#;

    #[test]
    fn projected_wkt1_should_be_projected() {
        assert_eq!(kind_from_wkt(UTM_WKT1), Some(CrsKind::Projected));
    }

    #[test]
    fn projected_wkt2_should_be_projected() {
        assert_eq!(kind_from_wkt(UTM_WKT2), Some(CrsKind::Projected));
    }

    #[test]
    fn geographic_wkt_should_be_geographic() {
        assert_eq!(kind_from_wkt(GEOGRAPHIC_WKT1), Some(CrsKind::Geographic));
        assert_eq!(kind_from_wkt(GEOGRAPHIC_WKT2), Some(CrsKind::Geographic));
    }

    #[test]
    fn compound_wkt_should_follow_its_horizontal_crs() {
        let wkt1 = format!(
            r#"COMPD_CS["UTM 55S + AHD height",{UTM_WKT1},VERT_CS["AHD height",VERT_DATUM["Australian Height Datum",2005],UNIT["metre",1]]]"#
        );
        assert_eq!(kind_from_wkt(&wkt1), Some(CrsKind::Projected));
        let wkt2 = format!(
            r#"COMPOUNDCRS["WGS 84 + EGM96 height",{GEOGRAPHIC_WKT2},VERTCRS["EGM96 height",VDATUM["EGM96 geoid"]]]"#
        );
        assert_eq!(kind_from_wkt(&wkt2), Some(CrsKind::Geographic));
    }

    #[test]
    fn bound_wkt2_should_follow_its_source_crs() {
        let wkt = format!(r#"BOUNDCRS[SOURCECRS[{UTM_WKT2}],TARGETCRS[{GEOGRAPHIC_WKT2}]]"#);
        assert_eq!(kind_from_wkt(&wkt), Some(CrsKind::Projected));
    }

    #[test]
    fn vertical_only_wkt_should_be_other() {
        let wkt = r#"VERTCRS["AHD height",VDATUM["Australian Height Datum"]]"#;
        assert_eq!(kind_from_wkt(wkt), Some(CrsKind::Other));
    }

    #[test]
    fn missing_or_empty_crs_should_be_none() {
        assert_eq!(kind_from_wkt(""), None);
        assert_eq!(crs_kind(None, None), None);
        assert_eq!(crs_kind(Some(b"\0\0\0"), None), None);
    }

    #[test]
    fn crs_kind_should_read_null_terminated_wkt() {
        let bytes = format!("{UTM_WKT1}\0");
        assert_eq!(
            crs_kind(Some(bytes.as_bytes()), None),
            Some(CrsKind::Projected)
        );
    }

    fn geokeys(keys: &[[u16; 4]]) -> Vec<u8> {
        let mut shorts = vec![1, 1, 0, keys.len() as u16];
        shorts.extend(keys.iter().flatten());
        shorts.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn geokeys_should_give_the_model_type() {
        let projected = geokeys(&[[1024, 0, 1, 1], [3072, 0, 1, 32755]]);
        assert_eq!(kind_from_geokeys(&projected), Some(CrsKind::Projected));
        let geographic = geokeys(&[[1024, 0, 1, 2], [2048, 0, 1, 4326]]);
        assert_eq!(kind_from_geokeys(&geographic), Some(CrsKind::Geographic));
        let only_projected_type = geokeys(&[[3072, 0, 1, 32755]]);
        assert_eq!(
            kind_from_geokeys(&only_projected_type),
            Some(CrsKind::Projected)
        );
        assert_eq!(kind_from_geokeys(&geokeys(&[])), None);
        assert_eq!(kind_from_geokeys(&[]), None);
    }

    #[test]
    fn crs_kind_should_fall_back_to_geokeys() {
        let projected = geokeys(&[[1024, 0, 1, 1]]);
        assert_eq!(crs_kind(None, Some(&projected)), Some(CrsKind::Projected));
    }
}
