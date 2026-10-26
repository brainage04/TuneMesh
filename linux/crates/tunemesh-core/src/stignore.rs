//! `<root>/.stignore` (contract: "`.stignore`"): the Syncthing ignore rules
//! every device needs so partial files are never synced.

use std::io;
use std::path::Path;

use crate::fsx::Dir;

pub const FILE_NAME: &str = ".stignore";
const HEADER: &str = "// TuneMesh";
/// The rules from `docs/library-format.md`.
pub const RULES: &[&str] = &["(?d).~tunemesh-*"];

/// Makes sure `<root>/.stignore` contains every rule in `RULES`: missing
/// ones are appended (exact match after trimming), the user's lines and
/// their order are kept, and a `// TuneMesh` header is added only when the
/// file is created. Nothing is written when no rule is missing. A
/// `.stignore` that is a symlink or not a regular file is refused; one over
/// `MAX_STIGNORE_BYTES` is left unchanged, with an error saying so and
/// which rules to add by hand (callers log it). Returns whether the file
/// was written.
pub fn ensure(root: &Path) -> io::Result<bool> {
    let dir = Dir::open(root)?;
    let max = crate::limits::MAX_STIGNORE_BYTES;
    if dir.file_len(FILE_NAME).is_ok_and(|len| len > max) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{FILE_NAME} is larger than {max} bytes, so it is left unchanged; add these \
                 lines to it by hand if they are missing: {}",
                RULES.join(" ")
            ),
        ));
    }
    let existing = match dir.read(FILE_NAME, max) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    match with_rules(existing.as_deref()) {
        Some(text) => dir.write_atomic(FILE_NAME, &text).map(|()| true),
        None => Ok(false),
    }
}

/// The `.stignore` contents `ensure` writes for `existing` (`None`: no
/// file), or `None` when no rule is missing. The existing bytes are kept
/// as they are, invalid UTF-8 included.
pub fn with_rules(existing: Option<&[u8]>) -> Option<Vec<u8>> {
    let present = |rule: &str| {
        existing.is_some_and(|t| {
            t.split(|b| *b == b'\n')
                .any(|line| String::from_utf8_lossy(line).trim() == rule)
        })
    };
    let missing: Vec<&str> = RULES.iter().copied().filter(|r| !present(r)).collect();
    if missing.is_empty() {
        return None;
    }
    let mut text = match existing {
        Some(t) => {
            let extra: usize = missing.iter().map(|r| r.len() + 1).sum();
            let mut out = Vec::with_capacity(t.len() + 1 + extra);
            out.extend_from_slice(t);
            if !out.is_empty() && !out.ends_with(b"\n") {
                out.push(b'\n');
            }
            out
        }
        None => format!("{HEADER}\n").into_bytes(),
    };
    for r in missing {
        text.extend_from_slice(r.as_bytes());
        text.push(b'\n');
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn creates_appends_and_keeps_user_lines() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        assert!(ensure(root).unwrap());
        assert_eq!(
            fs::read_to_string(root.join(FILE_NAME)).unwrap(),
            "// TuneMesh\n(?d).~tunemesh-*\n"
        );
        assert!(!ensure(root).unwrap());

        fs::write(root.join(FILE_NAME), "#include shared\n*.tmp").unwrap();
        assert!(ensure(root).unwrap());
        assert_eq!(
            fs::read_to_string(root.join(FILE_NAME)).unwrap(),
            "#include shared\n*.tmp\n(?d).~tunemesh-*\n"
        );
        fs::write(root.join(FILE_NAME), "  (?d).~tunemesh-*  \n").unwrap();
        assert!(!ensure(root).unwrap());
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn refuses_a_symlinked_stignore() {
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("x");
        fs::write(&target, "keep").unwrap();
        let d = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&target, d.path().join(FILE_NAME)).unwrap();
        assert!(ensure(d.path()).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    }

    /// Round 3 (trivial): an oversize `.stignore` was left alone with only
    /// a bare read error; the error now says it is left unchanged and what
    /// to add.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn oversize_files_are_left_alone_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let big = vec![b'x'; crate::limits::MAX_STIGNORE_BYTES as usize + 1];
        fs::write(&path, &big).unwrap();
        let e = ensure(dir.path()).unwrap_err();
        let msg = e.to_string();
        assert!(
            msg.contains("left unchanged") && msg.contains(RULES[0]),
            "{msg}"
        );
        assert_eq!(fs::read(&path).unwrap(), big);
    }

    /// Found by `fuzz/fuzz_targets/stignore.rs`: a `.stignore` with invalid
    /// UTF-8 had it replaced by U+FFFD when the rule was appended.
    #[test]
    fn keeps_the_users_bytes() {
        let old: &[u8] = b"caf\xe9/\n\xff*.tmp";
        let new = with_rules(Some(old)).unwrap();
        assert_eq!(new, b"caf\xe9/\n\xff*.tmp\n(?d).~tunemesh-*\n");
        assert_eq!(with_rules(Some(&new[..])), None);
        let present: &[u8] = b"\xff\r\n (?d).~tunemesh-*\r\n";
        assert_eq!(with_rules(Some(present)), None);
    }
}
