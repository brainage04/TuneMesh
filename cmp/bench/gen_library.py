#!/usr/bin/env python3
"""Generates a synthetic TuneMesh library (docs/library-format.md) for benchmarks.

150 artists x 2 albums x 10 tracks = 3000 short tracks: mostly Opus, some AAC
(.m4a) and MP3, a few albums with an embedded cover and a few with cover.jpg,
plus playlists (Liked Songs and eight others, one with missing entries).
Audio is generated sine/noise only.

usage: gen_library.py <root> [--seconds 20] [--jobs 12]
"""

import argparse
import base64
import os
import random
import struct
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor

SYLLABLES = ["ka", "lo", "mi", "ra", "ve", "su", "no", "tar", "el", "quin", "dra", "fen", "os", "ly", "zé", "ö", "ber", "an", "tic", "mo"]
WORDS = ["Night", "Echo", "Glass", "River", "Static", "Gold", "Velvet", "Signal", "Paper", "Neon", "Winter", "Ghost",
         "Orbit", "Salt", "Ember", "Hollow", "Silver", "Tide", "Atlas", "Mirror", "Lantern", "Cobalt", "Fever", "Dune"]


def name(rng, parts):
    return "".join(rng.choice(SYLLABLES) for _ in range(parts)).capitalize()


def phrase(rng, words):
    return " ".join(rng.choice(WORDS) for _ in range(words))


def sanitize(component):
    out = "".join("_" if c in '/\\:*?"<>|' or ord(c) < 0x20 else c for c in component)
    if out.startswith("."):
        out = "_" + out[1:]
    return out.rstrip(". ") or "_"


def flac_picture_block(jpeg, width, height):
    mime = b"image/jpeg"
    return (struct.pack(">II", 3, len(mime)) + mime + struct.pack(">I", 0)
            + struct.pack(">IIIII", width, height, 24, 0, len(jpeg)) + jpeg)


def make_cover(path, seed):
    sources = ["testsrc2", "mandelbrot", "cellauto", "life", "smptehdbars", "rgbtestsrc"]
    src = sources[seed % len(sources)]
    extra = {"cellauto": f"=rule={18 + seed % 100}:s=600x600", "life": f"=seed={seed}:s=600x600:mold=10",
             "mandelbrot": f"=s=600x600:start_scale={1 + seed % 7}"}.get(src, "=s=600x600")
    hue = (seed * 37) % 360
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", f"{src}{extra}", "-frames:v", "1",
                    "-vf", f"scale=600:600,hue=h={hue}", "-q:v", "4", path], check=True)


def encode(job):
    out, seconds, freq, noise, tags, cover, kind = job
    src = f"anoisesrc=d={seconds}:c=pink:a=0.2" if noise else f"sine=frequency={freq}:duration={seconds}"
    cmd = ["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", src]
    meta_file = None
    if cover and kind != "opus":
        cmd += ["-i", cover, "-map", "0:a", "-map", "1:v", "-c:v", "mjpeg", "-disposition:v", "attached_pic"]
    elif cover:
        # The base64 picture is too long for one command-line argument; pass it as an ffmetadata file.
        with open(cover, "rb") as f:
            block = base64.b64encode(flac_picture_block(f.read(), 600, 600)).decode().replace("=", "\\=")
        meta_file = out + ".ffmeta"
        with open(meta_file, "w") as f:
            f.write(f";FFMETADATA1\nMETADATA_BLOCK_PICTURE={block}\n")
        cmd += ["-f", "ffmetadata", "-i", meta_file, "-map", "0:a", "-map_metadata", "1"]
    if kind == "opus":
        cmd += ["-c:a", "libopus", "-b:a", "24k", "-ac", "1"]
    elif kind == "m4a":
        cmd += ["-c:a", "aac", "-b:a", "48k", "-ac", "1"]
    else:
        cmd += ["-c:a", "libmp3lame", "-b:a", "48k", "-ac", "1", "-id3v2_version", "3"]
    for key, value in tags.items():
        cmd += ["-metadata", f"{key}={value}"]
    cmd.append(out)
    try:
        subprocess.run(cmd, check=True)
    finally:
        if meta_file:
            os.remove(meta_file)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("root")
    parser.add_argument("--seconds", type=int, default=20)
    parser.add_argument("--jobs", type=int, default=12)
    parser.add_argument("--artists", type=int, default=150)
    args = parser.parse_args()

    rng = random.Random(20261019)
    root = os.path.abspath(args.root)
    os.makedirs(os.path.join(root, "Playlists"), exist_ok=True)
    covers = tempfile.mkdtemp(prefix="tm-covers-", dir=os.path.dirname(root))

    artists = []
    seen = set()
    while len(artists) < args.artists:
        candidate = f"{name(rng, 2)} {name(rng, rng.choice([1, 2]))}" if rng.random() < 0.6 else f"The {phrase(rng, 1)}s"
        if candidate.lower() not in seen:
            seen.add(candidate.lower())
            artists.append(candidate)

    jobs, tracks = [], []
    album_index = 0
    for a, artist in enumerate(artists):
        for _ in range(2):
            album = phrase(rng, rng.choice([1, 2, 3]))
            kind = "m4a" if album_index % 20 == 3 else "mp3" if album_index % 20 == 13 else "opus"
            embedded = album_index % 15 == 0
            folder_cover = album_index % 15 == 7
            album_dir = os.path.join(root, sanitize(artist), sanitize(album))
            os.makedirs(album_dir, exist_ok=True)
            cover = None
            if embedded or folder_cover:
                cover = os.path.join(covers, f"{album_index}.jpg")
                make_cover(cover, album_index)
                if folder_cover:
                    os.replace(cover, os.path.join(album_dir, "cover.jpg"))
                    cover = None
            mtime = 1_780_000_000 + album_index * 3600
            for n in range(1, 11):
                title = phrase(rng, rng.choice([1, 2, 3, 4]))
                track_artist = artist
                if rng.random() < 0.08:
                    track_artist = f"{artist}; {rng.choice(artists)}"
                source_id = "".join(rng.choice("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-") for _ in range(11))
                file_name = sanitize(f"{title} [{source_id}]") + "." + kind
                path = os.path.join(album_dir, file_name)
                tags = {"title": title, "artist": track_artist, "album": album, "album_artist": artist,
                        "track": str(n), "disc": "1", "date": str(1990 + album_index % 35)}
                jobs.append((path, args.seconds, 110 + (album_index * 10 + n) % 800, n % 4 == 0, tags,
                             cover if embedded else None, kind))
                tracks.append((os.path.relpath(path, root), track_artist, title, mtime + n))
            album_index += 1

    done = 0
    with ThreadPoolExecutor(args.jobs) as pool:
        for _ in pool.map(encode, jobs):
            done += 1
            if done % 250 == 0:
                print(f"{done}/{len(jobs)}", file=sys.stderr)
    for rel, _, _, mtime in tracks:
        os.utime(os.path.join(root, rel), (mtime, mtime))
    for leftover in os.listdir(covers):
        os.remove(os.path.join(covers, leftover))
    os.rmdir(covers)

    def write_playlist(pl_name, entries, missing=0):
        lines = ["#EXTM3U"]
        for rel, artist, title, _ in entries:
            lines += [f"#EXTINF:{args.seconds},{artist} - {title}", f"../{rel}"]
        for m in range(missing):
            lines += [f"#EXTINF:-1,Gone - Missing {m}", f"../Gone/Nowhere/Missing {m} [missing{m:04d}].opus"]
        with open(os.path.join(root, "Playlists", sanitize(pl_name) + ".m3u8"), "w", encoding="utf-8", newline="\n") as f:
            f.write("\n".join(lines) + "\n")

    def pick(count):
        return rng.sample(tracks, min(count, len(tracks)))

    write_playlist("Liked Songs", pick(200))
    write_playlist("Road Trip", pick(1000))
    write_playlist("Focus", pick(80), missing=3)
    write_playlist("Evening Mix", pick(45))
    write_playlist("Workout", pick(120))
    write_playlist("Rainy Days", pick(30))
    write_playlist("Late Night Café", pick(60))
    write_playlist("Old Favourites", pick(150))
    write_playlist("Empty Playlist", [])
    with open(os.path.join(root, ".stignore"), "w") as f:
        f.write("// TuneMesh\n(?d).~tunemesh-*\n")
    print(f"{len(tracks)} tracks, {album_index} albums, {len(artists)} artists in {root}")


if __name__ == "__main__":
    main()
