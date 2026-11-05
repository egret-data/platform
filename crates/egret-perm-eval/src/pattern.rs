//! Ranger wildcard matching, as the Trino service-def's `matcherOptions` say
//! (`wildCard: true, ignoreCase: true`): `*` is any run of characters
//! (possibly empty), `?` is exactly one, and both sides are lowercased.

/// Whether `value` matches the Ranger wildcard `pattern`, ignoring case.
pub fn matches(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let v: Vec<char> = value.to_lowercase().chars().collect();
    glob(&p, &v)
}

/// Whether `pattern` holds a wildcard character.
pub fn is_wild(pattern: &str) -> bool {
    pattern.contains(['*', '?'])
}

/// Iterative glob with single-star backtracking: linear in practice, never
/// exponential.
fn glob(p: &[char], v: &[char]) -> bool {
    let (mut pi, mut vi) = (0, 0);
    // The position after the last `*` seen, and the value position it was tried at.
    let mut star: Option<(usize, usize)> = None;
    while vi < v.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some((pi + 1, vi));
                pi += 1;
            }
            Some('?') => {
                pi += 1;
                vi += 1;
            }
            Some(c) if *c == v[vi] => {
                pi += 1;
                vi += 1;
            }
            _ => match star {
                // Let the last `*` swallow one more character and retry.
                Some((sp, sv)) => {
                    pi = sp;
                    vi = sv + 1;
                    star = Some((sp, sv + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn literals_compare_ignoring_case() {
        assert!(matches("Lake", "lAKE"));
        assert!(!matches("lake", "lakes"));
        assert!(!matches("lakes", "lake"));
    }

    #[test]
    fn star_is_any_run_including_none() {
        assert!(matches("*", ""));
        assert!(matches("*", "anything"));
        assert!(matches("sa*", "sa"));
        assert!(matches("sa*", "SALES"));
        assert!(matches("*es", "sales"));
        assert!(matches("s*l*s", "sales"));
        assert!(matches("a*b*c", "aXbYbZc"));
        assert!(!matches("sa*", "hr"));
        assert!(!matches("a*b", "acbc"));
    }

    #[test]
    fn question_mark_is_exactly_one_character() {
        assert!(matches("t?", "t1"));
        assert!(!matches("t?", "t"));
        assert!(!matches("t?", "t12"));
        assert!(matches("?*", "x"));
        assert!(!matches("?*", ""));
    }

    #[test]
    fn a_dollar_is_a_literal() {
        assert!(matches("t$*", "t$files"));
        assert!(!matches("t", "t$files"));
        assert!(matches("*", "t$files"));
    }
}
