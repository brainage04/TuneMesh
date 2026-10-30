//! A typical library for the stall walk: what one person's collection looks
//! like, as opposed to the hostile library at the contract's limits.
//!
//! 5,000 tracks in 400 albums by 150 artists; every track embeds its
//! album's 600 x 600 JPEG cover (about 170 KB) and has 2.6 s of silent
//! audio that mpv plays. 20 playlists of 20 to 500 entries, Liked Songs with
//! 300, and 30 downloads (done, failed, downloading, queued).

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

use spotless_core::hostile::{Tags, mp3};
use spotless_core::request::{Request, RequestState, Status};
use spotless_core::{LIKED_SONGS, PLAYLISTS_DIR};

pub const TRACKS: usize = 5_000;
pub const ALBUMS: usize = 400;
pub const ARTISTS: usize = 150;
pub const PLAYLISTS: usize = 20;
/// The longest playlist (the last one).
pub const BIG_PLAYLIST_ENTRIES: usize = 500;
pub const LIKED: usize = 300;
pub const DOWNLOADS: usize = 30;

/// MPEG-1 Layer III frames of silence: 100 of them play for 2.6 s.
const FRAMES: usize = 100;

const WORDS: &[&str] = &[
    "love",
    "night",
    "summer",
    "river",
    "golden",
    "heart",
    "blue",
    "fire",
    "dream",
    "city",
    "light",
    "rain",
    "home",
    "wild",
    "midnight",
    "ocean",
    "stars",
    "silver",
    "road",
    "winter",
    "echo",
    "shadow",
    "dance",
    "morning",
    "lonely",
    "electric",
    "paper",
    "garden",
    "satellite",
    "highway",
];

fn words(rng: &mut fastrand::Rng, n: usize) -> String {
    let mut out = String::new();
    for i in 0..n {
        let w = WORDS[rng.usize(..WORDS.len())];
        if i > 0 {
            out.push(' ');
            out.push_str(w);
        } else {
            let mut c = w.chars();
            if let Some(f) = c.next() {
                out.extend(f.to_uppercase());
                out.push_str(c.as_str());
            }
        }
    }
    out
}

pub fn artist(a: usize) -> String {
    format!("Artist {a:03}")
}

pub fn album(i: usize) -> String {
    format!("Album {i:03}")
}

/// Tracks of album `i`: 13 for the first half, 12 for the rest.
pub fn album_len(i: usize) -> usize {
    if i < ALBUMS / 2 { 13 } else { 12 }
}

/// Root-relative path of track `t` (1-based) of album `i`.
pub fn track_path(i: usize, t: usize) -> String {
    format!("{}/{}/{t:02}.mp3", artist(i % ARTISTS), album(i))
}

/// Every track path, album by album.
pub fn all_tracks() -> Vec<String> {
    (0..ALBUMS)
        .flat_map(|i| (1..=album_len(i)).map(move |t| track_path(i, t)))
        .collect()
}

pub fn playlist_name(p: usize) -> String {
    format!("Playlist {p:02}")
}

/// Entries of playlist `p`: 20 to 500.
pub fn playlist_len(p: usize) -> usize {
    20 + (BIG_PLAYLIST_ENTRIES - 20) * p / (PLAYLISTS - 1)
}

/// A 600 x 600 JPEG of about 170 KB: a gradient with noise.
fn cover(i: usize) -> Vec<u8> {
    let mut rng = fastrand::Rng::with_seed(i as u64);
    let (r0, g0, b0) = (rng.u8(..), rng.u8(..), rng.u8(..));
    let img = image::RgbImage::from_fn(600, 600, |x, y| {
        let t = (x + y) as f32 / 1200.0;
        let n = rng.i16(-40..40);
        let c = |base: u8| ((base as f32 * (1.0 - t) + 40.0 * t) as i16 + n).clamp(0, 255) as u8;
        image::Rgb([c(r0), c(g0), c(b0)])
    });
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
        .encode_image(&img)
        .expect("encode cover");
    out
}

fn write_playlist(root: &Path, name: &str, paths: impl Iterator<Item = String>) -> io::Result<()> {
    let mut f = io::BufWriter::new(fs::File::create(
        root.join(PLAYLISTS_DIR).join(format!("{name}.m3u8")),
    )?);
    f.write_all(b"#EXTM3U\n")?;
    for p in paths {
        writeln!(f, "../{p}")?;
    }
    f.flush()
}

/// Generates the library under `root` (which should be empty).
pub fn generate(root: &Path) -> io::Result<()> {
    let mut rng = fastrand::Rng::with_seed(7);
    let mut silence = Vec::with_capacity(FRAMES * 417);
    for _ in 0..FRAMES {
        // 128 kbit/s, 44.1 kHz: 417-byte frames.
        silence.extend_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
        silence.resize(silence.len() + 413, 0);
    }
    for i in 0..ALBUMS {
        let art = cover(i);
        let (artist, album) = (artist(i % ARTISTS), album(i));
        fs::create_dir_all(root.join(&artist).join(&album))?;
        for t in 1..=album_len(i) {
            let n = 1 + rng.usize(..4);
            let title = words(&mut rng, n);
            let mut bytes = mp3(&Tags {
                title: &title,
                artists: &[&artist],
                album: &album,
                track: Some(t as u32),
                cover: Some(&art),
            });
            // `mp3` ends with two silent frames; 100 make 2.6 s of audio.
            bytes.truncate(bytes.len() - 2 * 417);
            bytes.extend_from_slice(&silence);
            fs::write(root.join(track_path(i, t)), bytes)?;
        }
    }

    fs::create_dir_all(root.join(PLAYLISTS_DIR))?;
    let tracks = all_tracks();
    assert_eq!(tracks.len(), TRACKS);
    for p in 0..PLAYLISTS {
        let n = playlist_len(p);
        let picks: Vec<String> = (0..n)
            .map(|_| tracks[rng.usize(..tracks.len())].clone())
            .collect();
        write_playlist(root, &playlist_name(p), picks.into_iter())?;
    }
    write_playlist(
        root,
        LIKED_SONGS,
        (0..LIKED).map(|k| tracks[k * tracks.len() / LIKED].clone()),
    )?;

    for d in 0..DOWNLOADS {
        let state = match d % 4 {
            0 => RequestState::Done,
            1 => RequestState::Failed,
            2 => RequestState::Downloading,
            _ => RequestState::Queued,
        };
        let input = format!(
            "https://music.youtube.com/playlist?list=OLAK5uy_{}",
            words(&mut rng, 2).replace(' ', "_")
        );
        let mut req = Request::new(&input, None, false, "laptop");
        req.id = spotless_core::request::Id::from_string(format!(
            "{}-{d:08x}",
            1_760_000_000_000u64 + d as u64 * 60_000
        ));
        let mut status = Status::new(&req, state);
        status.title = Some(spotless_core::request::Text::from_string(format!(
            "{} - {}",
            artist(d),
            words(&mut rng, 3)
        )));
        match state {
            RequestState::Done => {
                for t in 1..=album_len(d) {
                    status.tracks.push(&track_path(d, t));
                }
                if d % 8 == 0 {
                    status.add_failure(
                        words(&mut rng, 3),
                        "Video unavailable: this video is not available in your country".to_owned(),
                    );
                }
            }
            RequestState::Failed => {
                status.message = Some(spotless_core::bounded::Bounded::from_string(
                    "ERROR: unable to download webpage: HTTP Error 429: Too Many Requests"
                        .to_owned(),
                ));
            }
            RequestState::Downloading => {
                status.progress = Some(0.4);
                status.items_done = Some(3);
                status.items_total = Some(12);
                req.write(root)?;
            }
            _ => req.write(root)?,
        }
        status.write(root)?;
    }
    Ok(())
}
