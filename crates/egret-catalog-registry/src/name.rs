//! The registry name is the catalog's SQL identifier on every engine (spec D5),
//! so it follows the strictest rule both engines accept.

use std::sync::OnceLock;

/// Longest catalog name (the `registry.catalogs` CHECK allows 1 + 62).
pub const MAX_NAME_LEN: usize = 63;

/// `^[a-z][a-z0-9_]{0,62}$` — the rule `registry.catalogs.name` enforces.
pub fn is_valid_catalog_name(name: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z][a-z0-9_]{0,62}$").expect("static name pattern"))
        .is_match(name)
}

/// Spec §4.8: lowercase, any other character becomes `_`, a name that does not
/// start with a letter gets a `w_` prefix, then truncate to [`MAX_NAME_LEN`].
pub fn normalize_catalog_name(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !s.starts_with(|c: char| c.is_ascii_lowercase()) {
        s = format!("w_{s}");
    }
    // Every char is ASCII here, so a byte truncate cannot split one.
    s.truncate(MAX_NAME_LEN);
    s
}

/// `base` + `_<n>`, truncating `base` so the result still fits.
pub fn with_suffix(base: &str, n: u32) -> String {
    join_truncated(base, &n.to_string())
}

/// `base` + `_` + `tail`, truncated to [`MAX_NAME_LEN`] (the `_<engine>` suffix
/// §4.8.3 gives a same-name catalog with a different configuration). `tail`
/// itself is bounded too, keeping at least one character of `base`, so the
/// result always fits regardless of how long `tail` is.
pub fn join_truncated(base: &str, tail: &str) -> String {
    // Reserve at least 1 byte for `base` plus the `_` separator.
    let max_tail_len = MAX_NAME_LEN.saturating_sub(2);
    let mut tail = tail.to_string();
    tail.truncate(max_tail_len);
    let keep = MAX_NAME_LEN.saturating_sub(tail.len() + 1);
    let mut head = base.to_string();
    head.truncate(keep);
    format!("{head}_{tail}")
}

/// `base` when free, else the first free `base_2`, `base_3`, ….
pub fn first_free_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2u32..)
        .map(|n| with_suffix(base, n))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded range always yields a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kind_cluster_warehouse_names_normalise_to_distinct_identifiers() {
        assert_eq!(
            normalize_catalog_name("e2e-wh-1784519491585"),
            "e2e_wh_1784519491585"
        );
        assert_eq!(
            normalize_catalog_name("e2e-wh-1784567124897-p"),
            "e2e_wh_1784567124897_p"
        );
        assert_eq!(normalize_catalog_name("e2e-wh-gov16659"), "e2e_wh_gov16659");
        assert!(is_valid_catalog_name("e2e_wh_1784519491585"));
        assert!(!is_valid_catalog_name("e2e-wh-1784519491585"));
    }

    #[test]
    fn a_name_that_does_not_start_with_a_letter_gets_a_prefix() {
        assert_eq!(normalize_catalog_name("1st"), "w_1st");
        assert_eq!(normalize_catalog_name("-x"), "w__x");
        assert_eq!(normalize_catalog_name(""), "w_");
        assert_eq!(normalize_catalog_name("Sales Data"), "sales_data");
        assert_eq!(normalize_catalog_name("Kho dữ liệu"), "kho_d__li_u");
    }

    #[test]
    fn normalised_names_are_always_valid_and_fit() {
        let long = "A".repeat(90);
        let n = normalize_catalog_name(&long);
        assert_eq!(n.len(), MAX_NAME_LEN);
        assert!(is_valid_catalog_name(&n));
        assert!(!is_valid_catalog_name(&"a".repeat(64)));
        assert!(is_valid_catalog_name(&"a".repeat(63)));
    }

    #[test]
    fn suffixes_skip_taken_names_and_keep_the_length_limit() {
        let taken = ["sales", "sales_2"];
        assert_eq!(first_free_name("sales", |n| taken.contains(&n)), "sales_3");
        assert_eq!(first_free_name("fresh", |n| taken.contains(&n)), "fresh");
        let base = "b".repeat(63);
        let s = with_suffix(&base, 12);
        assert_eq!(s.len(), MAX_NAME_LEN);
        assert!(s.ends_with("_12"));
        assert_eq!(join_truncated("pg_main", "trino_b"), "pg_main_trino_b");
        assert_eq!(
            join_truncated(&"c".repeat(60), "engine").len(),
            MAX_NAME_LEN
        );
    }

    #[test]
    fn join_truncated_bounds_a_long_tail_too() {
        let long_tail = "t".repeat(90);
        let joined = join_truncated("base", &long_tail);
        assert!(joined.len() <= MAX_NAME_LEN);
        assert!(is_valid_catalog_name(&joined));

        let long_base = "b".repeat(90);
        let joined = join_truncated(&long_base, &long_tail);
        assert!(joined.len() <= MAX_NAME_LEN);
        assert!(is_valid_catalog_name(&joined));
    }
}
