//! Limits on everything read from the synced folder (contract: "Limits").
//!
//! Any device sharing the folder can write any file in it, so every reader
//! bounds what it reads, keeps and shows. The numbers are the contract's;
//! the desktop app, the worker and this crate use them from here only.

use std::borrow::Cow;

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

/// A request, status or `worker.json` file.
pub const MAX_JSON_BYTES: u64 = MIB as u64;
/// `<root>/.stignore`.
pub const MAX_STIGNORE_BYTES: u64 = MIB as u64;
/// A request's `input`; longer requests fail.
pub const MAX_INPUT_BYTES: usize = 8 * KIB;
/// A request's `playlist` name.
pub const MAX_REQUEST_PLAYLIST_BYTES: usize = KIB;
/// Request and status ids clients read (the newest).
pub const MAX_LISTED_REQUESTS: usize = 200;
/// A request or status id, which is a file name (the JSON `id` is ignored).
pub const MAX_ID_BYTES: usize = 255;
/// Request `requestedBy`, `worker.json` `device`.
pub const MAX_DEVICE_BYTES: usize = KIB;
/// Request `requestedAt`, status `updatedAt`, `worker.json` `heartbeatAt`.
pub const MAX_TIMESTAMP_BYTES: usize = 64;
/// `worker.json` `machine`, `instance` and `workerVersion`.
pub const MAX_WORKER_ID_BYTES: usize = 64;
/// Each status `tracks` path.
pub const MAX_TRACK_PATH_BYTES: usize = 4 * KIB;
/// Status warnings a listed download keeps (the rest is counted).
pub const MAX_LISTED_WARNINGS: usize = 3;

/// Status `title`, each warning, each failure's `item` and `error`.
pub const MAX_TEXT_BYTES: usize = KIB;
/// Status `message`.
pub const MAX_MESSAGE_BYTES: usize = 4 * KIB;
/// Status `warnings` entries.
pub const MAX_WARNINGS: usize = 50;
/// Status `failures` entries.
pub const MAX_FAILURES: usize = 200;
/// Status `tracks` entries.
pub const MAX_STATUS_TRACKS: usize = 10_000;

/// A playlist file.
pub const MAX_PLAYLIST_BYTES: u64 = 32 * MIB as u64;
/// Entries in one playlist.
pub const MAX_PLAYLIST_ENTRIES: usize = 250_000;
/// One playlist line, without its line ending.
pub const MAX_PLAYLIST_LINE_BYTES: usize = 32 * KIB;
/// Playlists listed (Liked Songs not counted).
pub const MAX_PLAYLISTS: usize = 1_000;
/// An `#EXTINF` label, when read and when written.
pub const MAX_LABEL_BYTES: usize = KIB;

/// A tag text field (title, artist, album, album artist).
pub const MAX_TAG_TEXT_BYTES: usize = KIB;
/// Artists in one tag.
pub const MAX_ARTISTS: usize = 32;
/// A single tag item (a picture or a text frame).
pub const MAX_TAG_ITEM_BYTES: usize = 16 * MIB;

/// An embedded or folder cover.
pub const MAX_COVER_BYTES: u64 = 16 * MIB as u64;
/// Cover width and height, as stated in the image header.
pub const MAX_COVER_SIDE: u32 = 5_000;

/// Folder levels scanned below the root (the root is level 0).
pub const MAX_DEPTH: usize = 64;
/// Entries read from one folder.
pub const MAX_DIR_ENTRIES: usize = 100_000;
/// Audio files in the library.
pub const MAX_TRACKS: usize = 250_000;

/// Display: a single-line text (titles, names, labels, paths), in chars.
pub const MAX_LINE_CHARS: usize = 300;
/// Display: a message or warning, in chars.
pub const MAX_TEXT_CHARS: usize = 2_000;
/// Display: lines of a message or warning.
pub const MAX_TEXT_LINES: usize = 20;
/// Display: rows of a list read from a synced file, before "and N more".
pub const MAX_SHOWN_ROWS: usize = 200;

/// `s` cut to at most `max` bytes at a character boundary.
pub fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Cuts `s` in place to at most `max` bytes at a character boundary.
pub fn cut_in_place(s: &mut String, max: usize) {
    let len = cut(s, max).len();
    s.truncate(len);
}

/// For display: one line of at most `MAX_LINE_CHARS` characters (line
/// breaks become spaces), ending in `…` when cut.
pub fn clip_line(s: &str) -> Cow<'_, str> {
    let over = s.chars().nth(MAX_LINE_CHARS).is_some();
    if !over && !s.contains(['\n', '\r']) {
        return Cow::Borrowed(s);
    }
    let mut out: String = s
        .chars()
        .take(if over {
            MAX_LINE_CHARS - 1
        } else {
            MAX_LINE_CHARS
        })
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    if over {
        out.push('…');
    }
    Cow::Owned(out)
}

/// For display: a message of at most `MAX_TEXT_CHARS` characters and
/// `MAX_TEXT_LINES` lines, ending in `…` when text was cut (blank lines
/// past the limit are dropped without one).
pub fn clip_text(s: &str) -> Cow<'_, str> {
    // Byte index where the text must end, if it is too long.
    let by_chars = s
        .char_indices()
        .nth(MAX_TEXT_CHARS)
        .and(s.char_indices().nth(MAX_TEXT_CHARS - 1))
        .map(|(i, _)| i);
    let by_lines = s
        .match_indices('\n')
        .nth(MAX_TEXT_LINES - 1)
        .map(|(i, _)| i);
    match by_chars.into_iter().chain(by_lines).min() {
        None => Cow::Borrowed(s),
        Some(end) => {
            let mut out = s[..end].trim_end().to_owned();
            if !s[end..].trim().is_empty() {
                out.push('…');
            }
            Cow::Owned(out)
        }
    }
}

/// Width and height stated in the header of a JPEG, PNG, WebP or GIF image.
pub fn image_size(b: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| {
        Some(u32::from(u16::from_be_bytes(
            b.get(i..i + 2)?.try_into().ok()?,
        )))
    };
    let le16 = |i: usize| {
        Some(u32::from(u16::from_le_bytes(
            b.get(i..i + 2)?.try_into().ok()?,
        )))
    };
    let be32 = |i: usize| Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?));
    let le24 = |i: usize| {
        let x = b.get(i..i + 3)?;
        Some(u32::from(x[0]) | u32::from(x[1]) << 8 | u32::from(x[2]) << 16)
    };
    if b.starts_with(b"\x89PNG\r\n\x1a\n") && b.get(12..16) == Some(b"IHDR") {
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return match b.get(12..16)? {
            b"VP8 " if b.get(23..26) == Some(b"\x9d\x01\x2a") => {
                Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff))
            }
            b"VP8L" if b.get(20) == Some(&0x2f) => {
                let bits = u32::from_le_bytes(b.get(21..25)?.try_into().ok()?);
                Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            _ => None,
        };
    }
    if b.starts_with(b"\xff\xd8") {
        // Walk the markers to the first start-of-frame.
        let mut i = 2;
        loop {
            while *b.get(i)? != 0xff {
                i += 1;
            }
            while *b.get(i)? == 0xff {
                i += 1;
            }
            let marker = *b.get(i)?;
            i += 1;
            match marker {
                0xd8 | 0x01 | 0xd0..=0xd7 => continue,
                0xd9 | 0xda => return None,
                0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                    return Some((be16(i + 5)?, be16(i + 3)?));
                }
                _ => i += be16(i)? as usize,
            }
        }
    }
    None
}

/// Whether a cover may be decoded and shown: at most `MAX_COVER_BYTES`, and
/// at most `MAX_COVER_SIDE` pixels per side according to its header.
pub fn cover_ok(bytes: &[u8]) -> bool {
    bytes.len() as u64 <= MAX_COVER_BYTES
        && image_size(bytes).is_some_and(|(w, h)| {
            (1..=MAX_COVER_SIDE).contains(&w) && (1..=MAX_COVER_SIDE).contains(&h)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_at_char_boundaries() {
        assert_eq!(cut("héllo", 2), "h");
        assert_eq!(cut("héllo", 3), "hé");
        assert_eq!(cut("abc", 10), "abc");
        let mut s = "ééé".to_owned();
        cut_in_place(&mut s, 5);
        assert_eq!(s, "éé");
    }

    #[test]
    fn clips_for_display() {
        assert!(matches!(clip_line("short"), Cow::Borrowed(_)));
        assert_eq!(clip_line("a\nb"), "a b");
        let long = "x".repeat(MAX_LINE_CHARS + 5);
        let c = clip_line(&long);
        assert_eq!(c.chars().count(), MAX_LINE_CHARS);
        assert!(c.ends_with('…'));
        assert_eq!(
            clip_line(&"y".repeat(MAX_LINE_CHARS)).chars().count(),
            MAX_LINE_CHARS
        );
        let many = "line\n".repeat(50);
        let t = clip_text(&many);
        assert_eq!(t.lines().count(), MAX_TEXT_LINES);
        assert!(t.ends_with('…'));
        let big = "z".repeat(10_000);
        let t = clip_text(&big);
        assert_eq!(t.chars().count(), MAX_TEXT_CHARS);
        assert!(matches!(clip_text("ok\nfine"), Cow::Borrowed(_)));
        assert!(matches!(
            clip_text(&"z".repeat(MAX_TEXT_CHARS)),
            Cow::Borrowed(_)
        ));
        // Found by `fuzz/fuzz_targets/text_limits.rs`: blank lines past the
        // limit were all kept (a 2,000-line label for a synced message).
        let blank = format!("a{}", "\n".repeat(1_500));
        assert_eq!(clip_text(&blank), "a");
        let full = format!("{}\n \n \n", "l\n".repeat(MAX_TEXT_LINES));
        let t = clip_text(&full);
        assert_eq!(t.lines().count(), MAX_TEXT_LINES);
        assert!(!t.ends_with('…'));
    }

    /// Headers as image libraries write them.
    pub(crate) fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut v =
            b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00".to_vec();
        v.extend_from_slice(b"\xff\xdb\x00\x04\x00\x00");
        v.extend_from_slice(b"\xff\xc0\x00\x11\x08");
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        v.extend_from_slice(b"\xff\xd9");
        v
    }

    #[test]
    fn reads_image_headers() {
        assert_eq!(image_size(&jpeg(640, 480)), Some((640, 480)));
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&3000u32.to_be_bytes());
        png.extend_from_slice(&2000u32.to_be_bytes());
        assert_eq!(image_size(&png), Some((3000, 2000)));
        let gif = b"GIF89a\x10\x00\x20\x00".to_vec();
        assert_eq!(image_size(&gif), Some((16, 32)));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend_from_slice(&[0x1f, 0x03, 0x00, 0x0f, 0x02, 0x00]);
        assert_eq!(image_size(&webp), Some((800, 528)));
        assert_eq!(image_size(b"not an image"), None);
        assert!(cover_ok(&jpeg(3000, 3000)));
        assert!(!cover_ok(&jpeg(20_000, 20_000)));
        assert!(!cover_ok(&jpeg(0, 10)));
        assert!(!cover_ok(b"garbage"));
    }

    /// The numbers are the contract's ("Limits"), shared with Android.
    /// cargo-mutants: no test noticed a changed constant.
    #[test]
    fn limits_match_the_contract() {
        assert_eq!(MAX_JSON_BYTES, 1 << 20);
        assert_eq!(MAX_STIGNORE_BYTES, 1 << 20);
        assert_eq!(MAX_INPUT_BYTES, 8 << 10);
        assert_eq!(MAX_REQUEST_PLAYLIST_BYTES, 1 << 10);
        assert_eq!(MAX_LISTED_REQUESTS, 200);
        assert_eq!(MAX_TEXT_BYTES, 1 << 10);
        assert_eq!(MAX_MESSAGE_BYTES, 4 << 10);
        assert_eq!(
            (MAX_WARNINGS, MAX_FAILURES, MAX_STATUS_TRACKS),
            (50, 200, 10_000)
        );
        assert_eq!(MAX_PLAYLIST_BYTES, 32 << 20);
        assert_eq!(MAX_PLAYLIST_ENTRIES, 250_000);
        assert_eq!(MAX_PLAYLIST_LINE_BYTES, 32 << 10);
        assert_eq!((MAX_PLAYLISTS, MAX_LABEL_BYTES), (1_000, 1 << 10));
        assert_eq!((MAX_TAG_TEXT_BYTES, MAX_ARTISTS), (1 << 10, 32));
        assert_eq!(MAX_TAG_ITEM_BYTES, 16 << 20);
        assert_eq!((MAX_COVER_BYTES, MAX_COVER_SIDE), (16 << 20, 5_000));
        assert_eq!(
            (MAX_DEPTH, MAX_DIR_ENTRIES, MAX_TRACKS),
            (64, 100_000, 250_000)
        );
        assert_eq!(
            (MAX_LINE_CHARS, MAX_TEXT_CHARS, MAX_TEXT_LINES),
            (300, 2_000, 20)
        );
        assert_eq!(MAX_SHOWN_ROWS, 200);
    }

    /// Every header layout `image_size` reads, and look-alikes it must
    /// not. cargo-mutants: the WebP VP8 and VP8L branches, 24-bit VP8X
    /// sizes and the JPEG marker walk were untested.
    #[test]
    fn reads_every_header_layout() {
        let riff = |chunk: &[u8], rest: &[u8]| {
            let mut v = b"RIFF\0\0\0\0WEBP".to_vec();
            v.extend_from_slice(chunk);
            v.extend_from_slice(b"\0\0\0\0");
            v.extend_from_slice(rest);
            v
        };
        // VP8 (lossy): frame tag, start code, 14-bit sizes plus scale bits.
        let vp8 = riff(b"VP8 ", b"\0\0\0\x9d\x01\x2a\x80\x42\xe0\xc1");
        assert_eq!(image_size(&vp8), Some((640, 480)));
        let bad_start = riff(b"VP8 ", b"\0\0\0\x9d\x01\x2b\x80\x42\xe0\xc1");
        assert_eq!(image_size(&bad_start), None);
        // VP8L (lossless): signature, then width - 1 and height - 1 in 14 bits.
        let bits: u32 = 799 | (599 << 14);
        let mut rest = vec![0x2f];
        rest.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(image_size(&riff(b"VP8L", &rest)), Some((800, 600)));
        rest[0] = 0x2e;
        assert_eq!(image_size(&riff(b"VP8L", &rest)), None);
        // VP8X: 24-bit sizes.
        let vp8x = riff(b"VP8X", b"\0\0\0\0\x00\x00\x01\x02\x00\x00");
        assert_eq!(image_size(&vp8x), Some((65_537, 3)));
        // Look-alikes: another RIFF format with a WebP-like chunk, IHDR
        // without the PNG signature.
        let avi = b"RIFF\0\0\0\0AVI VP8X\0\0\0\0\0\0\0\0\x01\0\0\x01\0\0";
        assert_eq!(image_size(avi), None);
        assert_eq!(image_size(b"not a png!!!IHDR\0\0\0\x01\0\0\0\x01"), None);

        // JPEG: junk between segments, fill bytes, standalone markers, a
        // Huffman table (0xc4, not a frame) before a progressive frame.
        let mut j = b"\xff\xd8".to_vec();
        j.extend_from_slice(b"\xff\xc4\x00\x08\x00\x01\x02\x03\x04\x05");
        j.extend_from_slice(b"\x00\x00\xff\xff\xd0\xff\x01");
        j.extend_from_slice(b"\xff\xc2\x00\x11\x08\x01\xe0\x02\x80\x03");
        assert_eq!(image_size(&j), Some((640, 480)));
        // Scan data or the end before any frame: no size.
        let sos = b"\xff\xd8\xff\xda\x00\x02\xff\xc0\x00\x11\x08\x01\xe0\x02\x80\x03";
        assert_eq!(image_size(sos), None);
        let eoi = b"\xff\xd8\xff\xd9\xff\xc0\x00\x11\x08\x01\xe0\x02\x80\x03";
        assert_eq!(image_size(eoi), None);
    }
}
