//! File and directory names written by TuneMesh (contract: "Audio files").

/// Maximum length of one path component in bytes.
pub const MAX_COMPONENT_BYTES: usize = 120;

fn is_forbidden(c: char) -> bool {
    matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
}

fn replace_chars(s: &str) -> String {
    s.chars()
        .map(|c| if is_forbidden(c) { '_' } else { c })
        .collect()
}

/// Cuts `s` to at most `max` bytes on a character boundary.
fn truncate_bytes(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
}

/// Replaces trailing `.`/space with `_` and a leading `.` (which would hide
/// the file from every reader) with `_`.
fn fix_ends(s: &mut String) {
    if s.starts_with('.') {
        s.replace_range(0..1, "_");
    }
    let keep = s.trim_end_matches(['.', ' ']).len();
    let trailing = s.len() - keep;
    if trailing > 0 {
        s.truncate(keep);
        s.extend(std::iter::repeat_n('_', trailing));
    }
}

/// Sanitises one path component (an artist, album or playlist name).
pub fn component(name: &str) -> String {
    let mut s = replace_chars(name.trim());
    truncate_bytes(&mut s, MAX_COMPONENT_BYTES);
    fix_ends(&mut s);
    if s.is_empty() { "_".to_owned() } else { s }
}

/// Sanitises a file name made of a free-form `stem` and a fixed `suffix`
/// (for example `" [dQw4w9WgXcQ].opus"`): the stem is shortened first so the
/// suffix, which carries the source id and extension, always survives.
pub fn file_name(stem: &str, suffix: &str) -> String {
    let suffix = replace_chars(suffix);
    let mut stem = replace_chars(stem.trim());
    if stem.starts_with('.') {
        stem.replace_range(0..1, "_");
    }
    if stem.is_empty() {
        stem.push('_');
    }
    truncate_bytes(
        &mut stem,
        MAX_COMPONENT_BYTES.saturating_sub(suffix.len()).max(1),
    );
    let mut name = stem + &suffix;
    truncate_bytes(&mut name, MAX_COMPONENT_BYTES);
    fix_ends(&mut name);
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_forbidden_and_control_characters() {
        assert_eq!(
            component(r#"AC/DC: "Live" <1991>?|*\"#),
            "AC_DC_ _Live_ _1991_____"
        );
        assert_eq!(component("tab\there\u{7}"), "tab_here_");
    }

    #[test]
    fn fixes_trailing_dot_space_and_leading_dot() {
        assert_eq!(component("Mr."), "Mr_");
        assert_eq!(component("...And Justice"), "_..And Justice");
        assert_eq!(component("  x  "), "x");
        assert_eq!(component(""), "_");
        assert_eq!(component("."), "_");
    }

    #[test]
    fn limits_component_to_120_bytes_on_char_boundary() {
        let long = "é".repeat(100); // 200 bytes
        let s = component(&long);
        assert!(s.len() <= MAX_COMPONENT_BYTES);
        assert_eq!(s, "é".repeat(60));
    }

    #[test]
    fn file_name_keeps_id_suffix() {
        let name = file_name(&"a".repeat(300), " [dQw4w9WgXcQ].opus");
        assert_eq!(name.len(), MAX_COMPONENT_BYTES);
        assert!(name.ends_with(" [dQw4w9WgXcQ].opus"));
        assert_eq!(file_name("What? Now.", ".m3u8"), "What_ Now..m3u8");
        assert_eq!(file_name("Song", " [x].mp3"), "Song [x].mp3");
    }

    #[test]
    fn sanitising_is_idempotent() {
        for s in ["Mr.", "a/b", " .x. ", "Ünïcødé ☃", &"ß".repeat(90)] {
            let once = component(s);
            assert_eq!(component(&once), once);
        }
    }
}
