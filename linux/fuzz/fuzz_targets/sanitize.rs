//! File and directory names TuneMesh writes (contract: "Audio files"):
//! whatever the metadata or playlist name, the result is a valid name.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tunemesh_core::playlist::sanitize_name;
use tunemesh_core::sanitize::{MAX_COMPONENT_BYTES, component, file_name};

fn assert_valid(name: &str, from: &str) {
    assert!(!name.is_empty(), "{from:?}");
    assert!(name.len() <= MAX_COMPONENT_BYTES, "{from:?} -> {name:?}");
    assert!(
        !name.contains(|c: char| "/\\:*?\"<>|".contains(c) || c.is_control()),
        "{from:?} -> {name:?}"
    );
    assert!(!name.starts_with('.'), "{from:?} -> {name:?}");
    assert!(!name.ends_with(['.', ' ']), "{from:?} -> {name:?}");
}

fuzz_target!(|i: (&str, &str)| {
    let (name, suffix) = i;
    let c = component(name);
    assert_valid(&c, name);
    assert_eq!(component(&c), c, "component is not idempotent for {name:?}");

    let f = file_name(name, suffix);
    assert_valid(&f, name);
    // The suffix (source id and extension) survives when it can.
    let plain = |c: char| c.is_ascii_alphanumeric() || " []-_".contains(c);
    if suffix.len() < MAX_COMPONENT_BYTES / 2
        && suffix.contains('.')
        && !suffix.ends_with(['.', ' '])
        && suffix.chars().all(|c| plain(c) || c == '.')
    {
        assert!(f.ends_with(suffix), "{name:?} + {suffix:?} -> {f:?}");
    }

    let p = sanitize_name(name);
    assert_valid(&format!("{p}.m3u8"), name);
    assert!(!p.is_empty());
    assert_eq!(
        sanitize_name(&p),
        p,
        "sanitize_name is not idempotent for {name:?}"
    );
});
