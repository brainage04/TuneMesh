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
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let present: Vec<&str> = existing
        .as_deref()
        .map(|t| t.lines().map(str::trim).collect())
        .unwrap_or_default();
    let missing: Vec<&str> = RULES
        .iter()
        .copied()
        .filter(|r| !present.contains(r))
        .collect();
    if missing.is_empty() {
        return Ok(false);
    }
    let mut text = match existing {
        Some(t) => {
            let mut t = t;
            if !t.is_empty() && !t.ends_with('\n') {
                t.push('\n');
            }
            t
        }
        None => format!("{HEADER}\n"),
    };
    for r in missing {
        text.push_str(r);
        text.push('\n');
    }
    dir.write_atomic(FILE_NAME, text.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
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
    fn refuses_a_symlinked_stignore() {
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("x");
        fs::write(&target, "keep").unwrap();
        let d = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&target, d.path().join(FILE_NAME)).unwrap();
        assert!(ensure(d.path()).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    }
}
