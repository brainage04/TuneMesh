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
/// `.stignore` that is a symlink or not a regular file is refused. Returns
/// whether the file was written.
pub fn ensure(root: &Path) -> io::Result<bool> {
    let dir = Dir::open(root)?;
    let existing = match dir.read(FILE_NAME, crate::limits::MAX_STIGNORE_BYTES) {
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
            let mut t = t.to_vec();
            if !t.is_empty() && !t.ends_with(b"\n") {
                t.push(b'\n');
            }
            t
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
