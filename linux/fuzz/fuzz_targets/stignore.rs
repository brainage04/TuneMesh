//! `.stignore` upkeep: the user's file is kept byte for byte with the rules
//! appended, and a second pass changes nothing.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tunemesh_core::stignore::{RULES, with_rules};

/// Whether a line of `text` is `rule`, surrounding white space ignored (as
/// Syncthing reads it).
fn has_rule(text: &[u8], rule: &str) -> bool {
    text.split(|b| *b == b'\n')
        .any(|line| String::from_utf8_lossy(line).trim() == rule)
}

fuzz_target!(|i: (bool, &[u8])| {
    let existing = i.0.then_some(i.1);
    match with_rules(existing) {
        None => {
            let text = existing.expect("a missing file always gets the rules");
            assert!(RULES.iter().all(|r| has_rule(text, r)));
        }
        Some(out) => {
            if let Some(old) = existing {
                assert!(out.starts_with(old), "the user's lines are not kept");
            }
            assert!(RULES.iter().all(|r| has_rule(&out, r)));
            assert_eq!(with_rules(Some(&out)), None, "not a fixpoint");
        }
    }
});
