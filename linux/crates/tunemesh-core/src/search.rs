//! Case- and diacritic-insensitive matching.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Folds `s` for matching: compatibility-decomposed, combining marks
/// removed, lowercased.
pub fn fold(s: &str) -> String {
    s.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// A parsed query: folded words that must all occur in a folded key.
#[derive(Debug, Clone, Default)]
pub struct Query {
    words: Vec<String>,
}

impl Query {
    pub fn new(text: &str) -> Self {
        Self {
            words: fold(text).split_whitespace().map(str::to_owned).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Whether every word occurs in `folded_key`.
    pub fn matches(&self, folded_key: &str) -> bool {
        !self.words.is_empty() && self.words.iter().all(|w| folded_key.contains(w.as_str()))
    }

    /// Ranking score for a match on `folded_name` (lower is better): exact
    /// name, then name prefix, then name contains the query, then anything else.
    pub fn rank(&self, folded_name: &str) -> u8 {
        let joined = self.words.join(" ");
        if folded_name == joined {
            0
        } else if folded_name.starts_with(&joined) {
            1
        } else if folded_name.contains(&joined) {
            2
        } else {
            3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_and_diacritics() {
        assert_eq!(fold("Beyoncé"), "beyonce");
        assert_eq!(fold("SIGUR RÓS"), "sigur ros");
        assert_eq!(fold("Ｆｕｌｌ"), "full");
    }

    #[test]
    fn query_requires_every_word() {
        let q = Query::new("ros sigur");
        assert!(q.matches(&fold("Sigur Rós – Hoppípolla")));
        assert!(!q.matches(&fold("Sigur")));
        assert!(!Query::new("  ").matches("anything"));
    }
}
