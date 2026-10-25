//! Cutting and clipping synced text, cover headers, search folding and the
//! file-name helpers: never panic, and the results are within the limits.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tunemesh_core::library::{has_line_break, source_id_from_file_name, title_from_file_name};
use tunemesh_core::limits::{
    MAX_COVER_SIDE, MAX_LINE_CHARS, MAX_TEXT_CHARS, MAX_TEXT_LINES, clip_line, clip_text, cover_ok,
    cut, cut_in_place, image_size,
};
use tunemesh_core::search::{Query, fold};

fuzz_target!(|data: &[u8]| {
    if let Some((w, h)) = image_size(data) {
        if cover_ok(data) {
            assert!((1..=MAX_COVER_SIDE).contains(&w) && (1..=MAX_COVER_SIDE).contains(&h));
        }
    } else {
        assert!(!cover_ok(data));
    }

    let s = String::from_utf8_lossy(data);
    let max = data.first().map_or(0, |b| usize::from(*b) * 3);
    let c = cut(&s, max);
    assert!(c.len() <= max && s.starts_with(c));
    assert!(s.len() <= max || c.len() + 4 > max, "cut more than needed");
    let mut owned = s.to_string();
    cut_in_place(&mut owned, max);
    assert_eq!(owned, c);

    let line = clip_line(&s);
    assert!(line.chars().count() <= MAX_LINE_CHARS);
    assert!(!has_line_break(&line));
    let text = clip_text(&s);
    assert!(text.chars().count() <= MAX_TEXT_CHARS);
    assert!(
        text.lines().count() <= MAX_TEXT_LINES,
        "{} lines",
        text.lines().count()
    );

    let q = Query::new(&s);
    let folded = fold(&s);
    if !q.is_empty() {
        assert!(q.matches(&folded), "{s:?} does not find itself");
    }
    let _ = q.rank(&folded);
    let _ = title_from_file_name(&s);
    let _ = source_id_from_file_name(&s);
});
