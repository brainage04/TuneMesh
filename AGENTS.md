# TuneMesh agent instructions

- `docs/library-format.md` is the contract between the Linux app, the Android
  app and the worker. Change it first, then every implementation, in the same
  change.
- `android/` follows `/home/thomas/01_TM/Coding/Android Apps/AGENTS.md`
  (Gradle wrapper, `android-cycle`, emulator then phone, device safety rules),
  even though this repository lives outside that directory.
- `linux/` builds through the root `flake.nix`; use `nix develop` for Cargo.
- Never test with copyrighted music fixtures in the repository. Generate test
  audio with ffmpeg.
- GUI verification on Linux runs in an isolated headless Wayland compositor
  (for example headless sway + grim), never by capturing the user's desktop.
  Route test audio to a PipeWire null sink.
