# VM tests

The container tests in `.github/workflows/release.yml` cannot boot a real
system. These tests boot each distribution from its official cloud image in
QEMU and check what only a real boot shows: the worker as a systemd user
service under a logind session, the app under a Wayland compositor with
PipeWire audio, snapd, Flatpak, and the NixOS module.

| Distribution | Packages |
| --- | --- |
| `ubuntu-24.04` | `.deb`, AppImage, Snap |
| `ubuntu-26.04` | `.deb`, AppImage, Snap (built on 24.04) |
| `debian-13` | `.deb` |
| `fedora` (newest stable release) | `.rpm`, Flatpak |
| `arch` | `tunemesh`, `tunemesh-bin` |
| NixOS | the `worker-module` nixosTest in `flake.nix` |

## What a run checks

`run.sh <distro> <package-file>...` boots one VM (2 vCPUs, 3 GiB, KVM when
`/dev/kvm` is usable, TCG emulation otherwise) with a cloud-init seed that
creates the user `tm` with lingering, then over SSH, which gives the tests a
logind session:

1. installs sway, PipeWire, WirePlumber and pipewire-pulse, and a PipeWire
   null sink `tm-null` as the only audio output (`guest.sh deps`);
2. for each package, one after another (`guest.sh test`):
   - installs it with the distribution's tool (`apt`, `dnf`,
     `pacman`, `snap`, `flatpak`; the AppImage is copied to `~/Applications`
     after installing the desktop libraries it expects from the system, which
     a server cloud image lacks) and prints both `--version`s;
   - copies a generated library (two tagged sine-tone Opus files made with
     ffmpeg and a playlist) to `~/Music/TuneMesh` and writes the app's
     `config.toml` where that format keeps it;
   - starts `tunemesh-worker` as a systemd user service (the packaged unit
     for `.deb`, `.rpm` and Arch; a transient `systemd-run --user` unit with
     the same command for AppImage, Snap and Flatpak, which ship no unit),
     checks `systemctl --user is-active` and a running `tunemesh-worker`
     process after 15 s with no restarts, runs `tunemesh-worker --self-test`,
     and stops it (with the `flatpak run`/`snap run` scope the program moves
     into, which stopping the unit alone does not reach);
   - starts headless sway and the app on it, waits for
     `io.github.brainage04.TuneMesh` and `org.mpris.MediaPlayer2.tunemesh`
     on the session bus, activates `app.play-route` (`home`: the whole
     library) through `org.gtk.Actions`, and requires MPRIS
     `PlaybackStatus` `Playing`, the `Position` advancing by at least 3 s in
     5 s, the generated track in `Metadata`, and a running TuneMesh stream
     linked to `tm-null` in `pw-dump`; then `app.play-pause` must pause;
   - takes a screenshot, then sends the app process SIGTERM: it must exit
     within 15 s, cleanly, and log no missing icons, GSettings schemas or GTK
     criticals;
   - uninstalls it and checks that no file of the package is left
     (`packaging/ci/check-removed.sh`), that systemd no longer knows the unit,
     that `~/snap/tunemesh` or `~/.var/app/<id>` are gone, and that no
     `tunemesh` or `tunemesh-worker` process is left.

Logs (system and user journal, per-package worker, app and sway logs, MPRIS
metadata, `pw-dump`, screenshot, the serial console) end up in
`$TM_VM_LOGS` (default `./vm-logs-<distro>`).

## In CI

`.github/workflows/vm.yml` runs one distribution per job, each on its own
GitHub-hosted runner, with a 30 minute timeout; the NixOS job runs
`nix build .#checks.x86_64-linux.worker-module`. It runs weekly, by hand
(optionally on the packages of an earlier run: input `run-id`), and on pushes
and pull requests touching `packaging/**`, the workflow, `flake.nix`, or the
app's `mpv.rs` and `player.rs`; those runs build the packages through
`build.yml`. On `v*` tags the Release workflow calls it with its own packages
and publishes only when it passes.

Images are cached with `actions/cache` under a key made of the image URL and
its checksum, and every download is checked against it. Logs are uploaded
as the artifact `vm-logs-<distro>` when a job fails.

## Image checksums and SSH trust

- Ubuntu (24.04 and 26.04), Debian and Fedora images are pinned in `images.sh`, URL and
  checksum both; the comment there says where new values come from and which
  signature to check when moving a pin.
- Arch is rolling and keeps old images only for a few months, so
  `images.sh` takes the current snapshot and requires a good signature on its
  checksum file from the release key in `keys/` (`gpgv`).
- `run.sh` makes the guest's SSH host key itself and hands it over in the
  cloud-init seed, so SSH checks the guest against that pinned key
  (`StrictHostKeyChecking=yes`). QEMU binds the SSH forward on a port picked
  just before; when someone else took the port first, QEMU fails and
  `run.sh` retries with another one (five attempts).

## Running it locally

One VM at a time, always through `heavy`, which waits for memory and a slot:

```sh
setsid --fork --wait heavy --mem 4G --label vm -- \
    packaging/vm/run.sh ubuntu-24.04 dist/tunemesh_0.1.0-1_amd64.deb
```

- `run.sh` refuses to start outside `heavy` on machines that have it, and
  refuses while another `run.sh` VM holds the lock (`/tmp/tunemesh-vm.lock`,
  `TM_VM_LOCK`). It never starts more than one VM; test several packages by
  passing them to the same run, and distributions one run after the other.
- Needs `qemu-system-x86_64`, `qemu-img`, `ssh`, `curl`, `ffmpeg`, `python3`,
  `flock`, `tar`, `gpg`, `gpgv` and one of `cloud-localds`, `xorriso`, `genisoimage` or
  `mkisofs` (on NixOS: `nix shell nixpkgs#xorriso -c packaging/vm/run.sh ...`).
- Images (300 MB to 700 MB each) are kept in `TM_VM_CACHE` (default
  `~/.cache/tunemesh-vm`), one per distribution; a download is tried twice at
  most. Delete the directory when done.
- Each VM gets a fresh copy-on-write disk, SSH client key and host key in a temporary
  directory that is removed afterwards (`TM_VM_KEEP=1` keeps it).
- `TM_VM_ACCEL=tcg` forces emulation.
- `TM_VM_HOLD=1` keeps the VM running after a failed package and prints the
  SSH command to inspect it; deleting the printed hold file shuts it down.
