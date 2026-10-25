//! Playlist edits on a parsed playlist behave like the same edits on a
//! plain list (the span storage in `playlist::Entries` is invisible).
#![no_main]

use std::collections::HashSet;
use std::path::Path;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use tunemesh_core::playlist::{Playlist, parse};

#[derive(Arbitrary, Debug)]
enum Op {
    Append(Vec<u8>),
    Like(u8),
    LikeAll(Vec<u8>),
    Unlike(u8),
    UnlikeAll(Vec<u8>),
    Remove(Vec<u16>),
    Move(u16, u16),
}

#[derive(Arbitrary, Debug)]
struct Input {
    text: String,
    /// Track paths the edits pick from (by index).
    pool: Vec<String>,
    ops: Vec<Op>,
}

/// One entry as the public API shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    track: Option<String>,
    key: String,
    label: Option<String>,
    duration: Option<u64>,
}

impl Entry {
    fn track(t: &str) -> Self {
        Self {
            track: Some(t.to_owned()),
            key: t.to_owned(),
            label: None,
            duration: None,
        }
    }
}

fn has_line_break(s: &str) -> bool {
    s.contains(['\n', '\r'])
}

/// `Entries::push` refuses paths over 64 KiB.
fn storable(t: &str) -> bool {
    !has_line_break(t) && t.len() <= usize::from(u16::MAX)
}

fn entries(p: &Playlist) -> Vec<Entry> {
    p.entries
        .iter()
        .map(|e| Entry {
            track: e.track().map(str::to_owned),
            key: e.key().to_owned(),
            label: e.label().map(str::to_owned),
            duration: e.duration(),
        })
        .collect()
}

fuzz_target!(|input: Input| {
    let Ok(parsed) = parse(&input.text, Path::new("/r")) else {
        return;
    };
    let mut p = Playlist {
        name: "x".to_owned(),
        entries: parsed,
    };
    let mut model = entries(&p);
    let pick = |i: &u8| -> Option<&str> {
        (!input.pool.is_empty()).then(|| input.pool[usize::from(*i) % input.pool.len()].as_str())
    };
    for op in &input.ops {
        match op {
            Op::Append(ix) => {
                let ts: Vec<&str> = ix.iter().filter_map(pick).collect();
                p.append(ts.iter().copied());
                model.extend(ts.iter().filter(|t| storable(t)).map(|t| Entry::track(t)));
            }
            Op::Like(i) => {
                let Some(t) = pick(i) else { continue };
                let liked = model.iter().any(|e| e.track.as_deref() == Some(t));
                p.like(t);
                if !liked && storable(t) {
                    model.insert(0, Entry::track(t));
                }
            }
            Op::LikeAll(ix) => {
                let ts: Vec<&str> = ix.iter().filter_map(pick).collect();
                let mut seen: HashSet<String> =
                    model.iter().filter_map(|e| e.track.clone()).collect();
                let new: Vec<Entry> = ts
                    .iter()
                    .filter(|t| !has_line_break(t) && seen.insert((**t).to_owned()))
                    .filter(|t| storable(t))
                    .map(|t| Entry::track(t))
                    .collect();
                p.like_all(ts.iter().copied());
                model.splice(0..0, new);
            }
            Op::Unlike(i) => {
                let Some(t) = pick(i) else { continue };
                p.unlike(t);
                model.retain(|e| e.track.as_deref() != Some(t));
            }
            Op::UnlikeAll(ix) => {
                let ts: Vec<&str> = ix.iter().filter_map(pick).collect();
                p.unlike_all(ts.iter().copied());
                model.retain(|e| e.track.as_deref().is_none_or(|t| !ts.contains(&t)));
            }
            Op::Remove(at) => {
                let at: Vec<usize> = at.iter().map(|&i| usize::from(i)).collect();
                p.remove_positions(&at);
                let mut i = 0;
                model.retain(|_| {
                    i += 1;
                    !at.contains(&(i - 1))
                });
            }
            Op::Move(from, to) => {
                let (from, to) = (usize::from(*from), usize::from(*to));
                p.move_entry(from, to);
                if from < model.len() && to < model.len() && from != to {
                    let e = model.remove(from);
                    model.insert(to, e);
                }
            }
        }
        assert_eq!(entries(&p), model, "after {op:?}");
        assert_eq!(p.entries.len(), model.len());
    }
    for t in &input.pool {
        assert_eq!(
            p.contains(t),
            model.iter().any(|e| e.track.as_deref() == Some(t))
        );
    }
});
