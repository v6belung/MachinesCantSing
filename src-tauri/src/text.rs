use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// lowercase, trim, collapse internal whitespace, strip diacritics.
/// Shared by artist_id derivation (media) and iTunes match-name comparison (itunes::search).
pub fn normalize_artist_name(name: &str) -> String {
    let stripped: String = name.nfkd().filter(|c| !is_combining_mark(*c)).collect();
    stripped
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn names_match(a: &str, b: &str) -> bool {
    normalize_artist_name(a) == normalize_artist_name(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_diacritics_and_case() {
        assert_eq!(normalize_artist_name("Beyoncé"), "beyonce");
        assert_eq!(normalize_artist_name("  Sigur   Rós  "), "sigur ros");
    }

    #[test]
    fn matches_ignoring_case_and_accents() {
        assert!(names_match("Beyoncé", "beyonce"));
        assert!(!names_match("Beyonce", "Beyond"));
    }
}
