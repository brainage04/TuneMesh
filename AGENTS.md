# TuneMesh agent instructions

- If `AGENTS.local.md` exists (git-ignored, machine-specific notes), read it
  too; it overrides this file.
- `docs/library-format.md` is the contract between the Linux app, the Android
  app and the worker. Change it first, then every implementation, in the same
  change.
- `android/` has its own `android/AGENTS.md`.
- `linux/` builds through the root `flake.nix`; use `nix develop` for Cargo.
  Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test` from `linux/` before committing.
- Packaging recipes live in `packaging/`, CI and releases in
  `.github/workflows/`.
- Never test with copyrighted music fixtures in the repository. Generate test
  audio with ffmpeg.
- Never test against a real library or the user's own Syncthing: start a
  throwaway Syncthing with its own `--home` and generate a library under
  `/tmp`.
- GUI verification on Linux runs in an isolated headless Wayland compositor
  (for example headless sway + grim), never by capturing the user's desktop.
  Route test audio to a PipeWire null sink.
