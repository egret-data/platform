//! One raw path, checked once, forwarded unchanged.
//!
//! S0's first proxy matched a NORMALISED path and forwarded one that the HTTP client
//! normalised again. Of 40 raw-socket variants, `GET /e/x/../api/packages/list` and its
//! `%2e%2e` / `..%2f` forms returned 200 with marimo's package list, and two
//! `POST …/x/../api/files/list_files` reached marimo. The fix is not a better
//! normaliser but to stop having two paths: the proxy takes the path exactly as the
//! client sent it, REJECTS any spelling that could mean something else after another
//! layer's decoding, and forwards that same string.
//!
//! [`check_raw_path`] therefore returns a rejection or the (percent-decoded,
//! lower-cased) form to MATCH against; the caller forwards the original `raw`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRejection {
    /// Not an origin-form path: must start with `/`.
    NotAbsolute,
    /// A bad escape (`%` not followed by two hex digits), invalid UTF-8 or a control character.
    Malformed,
    /// A `.` or `..` segment.
    DotSegment,
    /// An escaped `.` `/` `\` or `%`: no legitimate marimo path needs one, and each is a
    /// way to smuggle a separator past a layer that decodes once more than this one.
    EncodedSeparator,
    DoubleSlash,
    Nul,
    Backslash,
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `raw` is the request target's path exactly as received, without the query string.
pub fn check_raw_path(raw: &str) -> Result<String, PathRejection> {
    if !raw.starts_with('/') {
        return Err(PathRejection::NotAbsolute);
    }
    if raw.contains('\\') {
        return Err(PathRejection::Backslash);
    }
    if raw.contains('\0') {
        return Err(PathRejection::Nul);
    }
    // Decode one level, rejecting every escape that stands for something structural.
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex),
                bytes.get(i + 2).copied().and_then(hex),
            ) else {
                return Err(PathRejection::Malformed);
            };
            let byte = hi * 16 + lo;
            match byte {
                0 => return Err(PathRejection::Nul),
                b'.' | b'/' | b'\\' | b'%' => return Err(PathRejection::EncodedSeparator),
                b if b < 0x20 || b == 0x7f => return Err(PathRejection::Malformed),
                _ => decoded.push(byte),
            }
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    if raw.contains("//") {
        return Err(PathRejection::DoubleSlash);
    }
    // No escaped dot survives the loop above, so a dot segment can only be spelled
    // literally — which is also exactly what "after one percent-decode" would find.
    if raw.split('/').skip(1).any(|seg| seg == "." || seg == "..") {
        return Err(PathRejection::DotSegment);
    }
    let decoded = String::from_utf8(decoded).map_err(|_| PathRejection::Malformed)?;
    Ok(decoded.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The routes S0 tried to reach through the proxy, under each surface prefix.
    const BLOCKED: [&str; 4] = [
        "/api/packages/list",
        "/api/files/list_files",
        "/terminal/ws",
        "/api/secrets/keys",
    ];
    const PREFIXES: [&str; 2] = ["/e/x", "/r/x"];

    fn forms(prefix: &str, base: &str) -> Vec<String> {
        let rest = &base[1..];
        vec![
            format!("{prefix}/../{rest}"),
            format!("{prefix}/%2e%2e/{rest}"),
            format!("{prefix}/%2E%2E/{rest}"),
            format!("{prefix}/..%2f{rest}"),
            format!("{prefix}/..%2F{rest}"),
            format!("{prefix}/./{rest}"),
            format!("{prefix}//{rest}"),
            format!("{prefix}\\{rest}"),
            format!("{prefix}%5c{rest}"),
            format!("{prefix}%5C{rest}"),
            format!("{prefix}%00{base}"),
            format!("{prefix}/%252e%252e/{rest}"),
        ]
    }

    #[test]
    fn every_dot_slash_backslash_and_nul_form_of_a_blocked_route_is_rejected() {
        let mut checked = 0;
        for base in BLOCKED {
            for prefix in PREFIXES {
                for form in forms(prefix, base) {
                    assert!(check_raw_path(&form).is_err(), "{form} was accepted");
                    checked += 1;
                }
            }
        }
        assert_eq!(
            checked, 96,
            "the matrix is 4 routes × 2 prefixes × 12 forms"
        );
    }

    #[test]
    fn each_class_is_named() {
        use PathRejection::*;
        assert_eq!(check_raw_path("api/x"), Err(NotAbsolute));
        assert_eq!(check_raw_path("/a/../b"), Err(DotSegment));
        assert_eq!(check_raw_path("/a/./b"), Err(DotSegment));
        assert_eq!(check_raw_path("/a/%2e%2e/b"), Err(EncodedSeparator));
        assert_eq!(check_raw_path("/a%2Fb"), Err(EncodedSeparator));
        assert_eq!(check_raw_path("/a%5Cb"), Err(EncodedSeparator));
        assert_eq!(check_raw_path("/a%25b"), Err(EncodedSeparator));
        assert_eq!(check_raw_path("/a//b"), Err(DoubleSlash));
        assert_eq!(check_raw_path("/a\\b"), Err(Backslash));
        assert_eq!(check_raw_path("/a%00b"), Err(Nul));
        assert_eq!(
            check_raw_path("/a%0ab"),
            Err(Malformed),
            "a control character"
        );
        assert_eq!(check_raw_path("/a%zzb"), Err(Malformed));
        assert_eq!(check_raw_path("/a%2"), Err(Malformed));
        assert_eq!(check_raw_path("/a%"), Err(Malformed));
        assert_eq!(check_raw_path("/a%ffb"), Err(Malformed), "invalid UTF-8");
    }

    #[test]
    fn ordinary_marimo_paths_are_accepted_and_come_back_decoded_and_lower_cased() {
        assert_eq!(
            check_raw_path("/e/pod/api/kernel/save").unwrap(),
            "/e/pod/api/kernel/save"
        );
        assert_eq!(
            check_raw_path("/r/v1/@file/12-a%20b.png").unwrap(),
            "/r/v1/@file/12-a b.png"
        );
        assert_eq!(
            check_raw_path("/e/pod/assets/index-AbC.js").unwrap(),
            "/e/pod/assets/index-abc.js"
        );
        // case-insensitive matching: `/API/Packages/List` must not slip past `/api/packages`
        assert_eq!(
            check_raw_path("/e/pod/API/Packages/List").unwrap(),
            "/e/pod/api/packages/list"
        );
        assert_eq!(check_raw_path("/e/pod/").unwrap(), "/e/pod/");
        assert_eq!(check_raw_path("/").unwrap(), "/");
    }

    #[test]
    fn a_dot_inside_a_name_is_not_a_dot_segment() {
        assert!(check_raw_path("/a/.hidden").is_ok());
        assert!(check_raw_path("/a/..x").is_ok());
        assert!(check_raw_path("/a/b..").is_ok());
        assert!(check_raw_path("/a/b.c").is_ok());
    }
}
