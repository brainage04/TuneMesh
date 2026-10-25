{
  description = "TuneMesh: a Syncthing-backed personal music player and its download worker";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/c5c4a43b0e8056328ec4529f735cabdb8f1942bb";
  # Only for the `nightly` dev shell (fuzzing, Miri, sanitizers).
  inputs.rust-overlay = {
    url = "github:oxalica/rust-overlay";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      inherit (pkgs) lib;

      version = (lib.importTOML ./linux/Cargo.toml).workspace.package.version;

      src = lib.fileset.toSource {
        root = ./linux;
        fileset = lib.fileset.unions [
          ./linux/Cargo.toml
          ./linux/Cargo.lock
          ./linux/crates
          ./linux/data
        ];
      };

      # The app only needs libmpv's audio path; the stock build is used so the
      # binary cache serves it.
      libmpv = pkgs.mpv-unwrapped;

      tunemesh = pkgs.rustPlatform.buildRustPackage {
        pname = "tunemesh";
        inherit version src;
        cargoLock.lockFile = ./linux/Cargo.lock;
        cargoBuildFlags = [ "-p" "tunemesh" ];
        cargoTestFlags = [ "-p" "tunemesh-core" "-p" "tunemesh" ];

        nativeBuildInputs = with pkgs; [
          pkg-config
          wrapGAppsHook4
          glib # glib-compile-resources for build.rs
        ];
        buildInputs = with pkgs; [
          gtk4
          libadwaita
          libmpv
        ];

        # Symbolic icons (transport controls, navigation) come from Adwaita.
        preFixup = ''
          gappsWrapperArgs+=(--prefix XDG_DATA_DIRS : "${pkgs.adwaita-icon-theme}/share")
        '';

        postInstall = ''
          install -Dm644 data/io.github.brainage04.TuneMesh.desktop \
            $out/share/applications/io.github.brainage04.TuneMesh.desktop
          install -Dm644 data/io.github.brainage04.TuneMesh.svg \
            $out/share/icons/hicolor/scalable/apps/io.github.brainage04.TuneMesh.svg
          install -Dm644 data/io.github.brainage04.TuneMesh.metainfo.xml \
            $out/share/metainfo/io.github.brainage04.TuneMesh.metainfo.xml
          mkdir -p $out/lib/systemd/user
          substitute ${./packaging/systemd/tunemesh-worker.service.in} \
            $out/lib/systemd/user/tunemesh-worker.service --subst-var-by bindir $out/bin
          # TuneMesh's licence and the Rust crates linked into both binaries.
          install -Dm644 ${./LICENSE} $out/share/licenses/tunemesh/LICENSE
          install -Dm644 ${./packaging/licenses/THIRD-PARTY-RUST.txt} \
            $out/share/licenses/tunemesh/THIRD-PARTY-RUST.txt
        '';

        # The app starts the worker from its own bin directory. Linked in after
        # wrapGAppsHook has run, so the worker keeps only its own wrapper.
        postFixup = ''
          ln -s ${tunemesh-worker}/bin/tunemesh-worker $out/bin/tunemesh-worker
        '';

        meta = {
          description = "Desktop music player for a Syncthing music library";
          mainProgram = "tunemesh";
          platforms = [ system ];
        };
      };

      tunemesh-worker = pkgs.rustPlatform.buildRustPackage {
        pname = "tunemesh-worker";
        inherit version src;
        cargoLock.lockFile = ./linux/Cargo.lock;
        cargoBuildFlags = [ "-p" "tunemesh-worker" ];
        cargoTestFlags = [ "-p" "tunemesh-worker" ];

        nativeBuildInputs = [ pkgs.makeWrapper ];
        # tests/ffmpeg_confinement.rs runs the real ffmpeg.
        nativeCheckInputs = [ pkgs.ffmpeg ];

        # The worker finds yt-dlp, deno (yt-dlp's JavaScript runtime for
        # YouTube) and ffmpeg through these variables, so it never downloads
        # its own copies on NixOS.
        postInstall = ''
          wrapProgram $out/bin/tunemesh-worker \
            --set-default TUNEMESH_YTDLP ${lib.getExe pkgs.yt-dlp} \
            --set-default TUNEMESH_DENO ${lib.getExe pkgs.deno} \
            --set-default TUNEMESH_FFMPEG_DIR ${lib.getBin pkgs.ffmpeg}/bin
          install -Dm644 ${./LICENSE} $out/share/licenses/tunemesh-worker/LICENSE
          install -Dm644 ${./packaging/licenses/THIRD-PARTY-RUST.txt} \
            $out/share/licenses/tunemesh-worker/THIRD-PARTY-RUST.txt
        '';

        meta = {
          description = "TuneMesh download worker (yt-dlp)";
          mainProgram = "tunemesh-worker";
          platforms = [ system ];
        };
      };
    in
    {
      packages.${system} = {
        inherit tunemesh tunemesh-worker;
        default = tunemesh;
      };

      checks.${system} = {
        inherit tunemesh tunemesh-worker;
        # The app package carries everything the other packages do: the worker
        # next to the app (where the app looks for it) and the user unit.
        tunemesh-layout = pkgs.runCommand "tunemesh-layout" { } ''
          test -x ${tunemesh}/bin/tunemesh
          test -x ${tunemesh}/bin/tunemesh-worker
          test -f ${tunemesh}/share/applications/io.github.brainage04.TuneMesh.desktop
          test -f ${tunemesh}/share/icons/hicolor/scalable/apps/io.github.brainage04.TuneMesh.svg
          grep -qx 'ExecStart=${tunemesh}/bin/tunemesh-worker' \
            ${tunemesh}/lib/systemd/user/tunemesh-worker.service
          test -s ${tunemesh}/share/licenses/tunemesh/LICENSE
          test -s ${tunemesh}/share/licenses/tunemesh/THIRD-PARTY-RUST.txt
          touch $out
        '';

        # The NixOS module's sandbox: the worker service starts and becomes
        # the downloader, and a unit with exactly the worker's sandboxing
        # runs `tunemesh-worker --self-test` and deno. Needs KVM.
        worker-module = pkgs.testers.runNixOSTest {
          name = "tunemesh-worker-module";
          nodes.machine =
            { config, ... }:
            {
              imports = [ self.nixosModules.worker ];
              users.users.alice = {
                isNormalUser = true;
                uid = 1000;
              };
              # A library path with a space and `%` (escaping in the unit).
              systemd.tmpfiles.settings."10-tunemesh-test" = {
                "/home/alice/Music".d = {
                  user = "alice";
                  group = "users";
                  mode = "0755";
                };
                "/home/alice/Music/Tune Mesh 100%%".d = {
                  user = "alice";
                  group = "users";
                  mode = "0755";
                };
              };
              services.tunemesh-worker = {
                enable = true;
                user = "alice";
                libraryPath = "/home/alice/Music/Tune Mesh 100%";
                workDir = "/var/cache/tunemesh worker 100%";
              };
              systemd.services.tunemesh-sandbox-check = {
                inherit (config.systemd.services.tunemesh-worker) environment;
                serviceConfig = config.systemd.services.tunemesh-worker.serviceConfig // {
                  Type = "oneshot";
                  Restart = "no";
                  ExecStart = pkgs.writeShellScript "tunemesh-sandbox-check" ''
                    set -eu
                    ${lib.getExe tunemesh-worker} --self-test --work-dir ${lib.escapeShellArg config.services.tunemesh-worker.workDir}
                    export DENO_DIR="$XDG_CACHE_HOME/deno"
                    ${lib.getExe pkgs.deno} eval 'console.log("deno " + (6 * 7))'
                    echo "deno: $(echo 'console.log(Deno.version.v8)' | \
                      ${lib.getExe pkgs.deno} run --no-prompt --no-remote --no-code-cache --v8-flags=--jitless -)"
                  '';
                };
              };
            };
          testScript = ''
            machine.wait_for_unit("multi-user.target")
            machine.wait_until_succeeds(
                "journalctl -u tunemesh-worker | grep -q 'is now the downloader'", timeout=120
            )
            machine.succeed("test -s '/home/alice/Music/Tune Mesh 100%/.tunemesh/worker.json'")
            machine.succeed("systemctl start tunemesh-sandbox-check")
            out = machine.succeed("journalctl -u tunemesh-sandbox-check")
            print(out)
            assert "self-test passed" in out, "self-test did not pass"
            assert "deno 42" in out, "deno did not run"
            machine.fail("journalctl -b | grep -q 'status=31/SYS'")
          '';
        };
      };

      devShells.${system} = {
        default = pkgs.mkShell {
          inputsFrom = [
            tunemesh
            tunemesh-worker
          ];
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            yt-dlp
            ffmpeg
            cargo-deny
            cargo-mutants
          ];
          shellHook = ''
            export XDG_DATA_DIRS="${pkgs.adwaita-icon-theme}/share:${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}:$XDG_DATA_DIRS"
          '';
        };

        # Nightly Rust for the checks that need it: cargo-fuzz (linux/fuzz),
        # Miri and AddressSanitizer runs. Pinned to one dated nightly.
        nightly = pkgs.mkShell {
          packages = [
            ((rust-overlay.lib.mkRustBin { } pkgs).nightly."2026-10-20".default.override {
              extensions = [
                "miri"
                "rust-src"
              ];
            })
            pkgs.cargo-fuzz
            pkgs.ffmpeg
          ];
        };
      };

      nixosModules.worker =
        {
          config,
          lib,
          pkgs,
          utils,
          ...
        }:
        let
          cfg = config.services.tunemesh-worker;
          # One path in a systemd path list: quoted (spaces), with `%`
          # (specifiers), `"` and `\` escaped.
          systemdPath =
            path: ''"${lib.replaceStrings [ "\\" "\"" "%" ] [ "\\\\" "\\\"" "%%" ] path}"'';
        in
        {
          options.services.tunemesh-worker = {
            enable = lib.mkEnableOption "the TuneMesh download worker";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.tunemesh-worker;
              defaultText = lib.literalExpression "tunemesh.packages.\${system}.tunemesh-worker";
              description = "The tunemesh-worker package to run.";
            };
            user = lib.mkOption {
              type = lib.types.str;
              description = "User that owns the library folder; the worker runs as this user.";
            };
            libraryPath = lib.mkOption {
              type = lib.types.str;
              example = "/home/user/Music/TuneMesh";
              description = "Root of the TuneMesh library (the Syncthing folder).";
            };
            workDir = lib.mkOption {
              type = lib.types.str;
              default = "/var/cache/tunemesh-worker";
              description = ''
                Directory outside the library where downloads are assembled
                before they are moved into place. Created for `user` with mode
                0700; the worker refuses a work directory that other users can
                access or that it does not own.
              '';
            };
            extraArgs = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [ ];
              example = [
                "--max-items"
                "1000"
                "--min-free-space"
                "10G"
              ];
              description = "Extra `tunemesh-worker` options, such as its limits (see `tunemesh-worker --help`).";
            };
            memoryMax = lib.mkOption {
              type = lib.types.str;
              default = "2G";
              description = "systemd MemoryMax for the worker and its downloaders.";
            };
            allowedAddresses = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default =
                lib.optional config.services.resolved.enable "127.0.0.53/32"
                ++ lib.optionals config.services.tailscale.enable [
                  "100.100.100.100/32"
                  "fd7a:115c:a1e0::53/128"
                ];
              defaultText = lib.literalExpression ''
                lib.optional config.services.resolved.enable "127.0.0.53/32"
                ++ lib.optionals config.services.tailscale.enable [ "100.100.100.100/32" "fd7a:115c:a1e0::53/128" ]
              '';
              description = ''
                DNS resolvers in a blocked range that the worker must reach to
                resolve host names: the systemd-resolved stub and Tailscale
                MagicDNS when those are enabled. Add a LAN resolver here only
                if the host sends DNS to one directly (with nscd, the NixOS
                default, lookups go through nscd's socket and need no entry).

                systemd cannot limit these entries to port 53: every port on
                them is reachable from the service. Only the worker's own
                resolver lookups use them; its egress proxy refuses them (and
                every other non-public address) as download targets.
              '';
            };
          };

          config = lib.mkIf cfg.enable {
            # tmpfiles expands `%` specifiers in paths.
            systemd.tmpfiles.settings."10-tunemesh-worker".${lib.replaceStrings [ "%" ] [ "%%" ] cfg.workDir}.d = {
              inherit (cfg) user;
              group = config.users.users.${cfg.user}.group;
              mode = "0700";
            };

            systemd.services.tunemesh-worker = {
              description = "TuneMesh download worker";
              wantedBy = [ "multi-user.target" ];
              wants = [ "network-online.target" ];
              after = [ "network-online.target" ];
              # Caches live in the work directory, the only writable place
              # besides the library.
              # systemd expands `%` specifiers in Environment= values.
              environment = lib.mapAttrs (_: lib.replaceStrings [ "%" ] [ "%%" ]) {
                XDG_CACHE_HOME = "${cfg.workDir}/.cache";
                XDG_CONFIG_HOME = "${cfg.workDir}/.config";
                XDG_DATA_HOME = "${cfg.workDir}/.local/share";
              };
              serviceConfig = {
                User = cfg.user;
                Group = config.users.users.${cfg.user}.group;
                # systemd expands % and $ in ExecStart; this escapes both.
                ExecStart = utils.escapeSystemdExecArgs (
                  [
                    (lib.getExe cfg.package)
                    "--library"
                    cfg.libraryPath
                    "--work-dir"
                    cfg.workDir
                  ]
                  ++ cfg.extraArgs
                );
                # Exit code 2 means a configuration error: retrying will not help.
                Restart = "on-failure";
                RestartPreventExitStatus = 2;
                RestartSec = 10;
                # Files written into the Syncthing folder stay group-readable
                # like the user's own files.
                UMask = "0022";
                Nice = 10;
                IOSchedulingClass = "idle";
                MemoryMax = cfg.memoryMax;

                # The worker runs downloaders on input any synced device can
                # write: it may touch the library and the work directory only.
                ProtectSystem = "strict";
                ProtectHome = "read-only";
                ReadWritePaths = map systemdPath [
                  cfg.libraryPath
                  cfg.workDir
                ];
                PrivateTmp = true;
                PrivateDevices = true;
                PrivateIPC = true;
                NoNewPrivileges = true;
                CapabilityBoundingSet = "";
                AmbientCapabilities = "";
                ProtectKernelTunables = true;
                ProtectKernelModules = true;
                ProtectKernelLogs = true;
                ProtectControlGroups = true;
                ProtectClock = true;
                ProtectHostname = true;
                ProtectProc = "invisible";
                LockPersonality = true;
                RestrictRealtime = true;
                RestrictSUIDSGID = true;
                RestrictNamespaces = true;
                RestrictAddressFamilies = [
                  "AF_INET"
                  "AF_INET6"
                  "AF_UNIX"
                  "AF_NETLINK"
                ];
                SystemCallArchitectures = "native";
                SystemCallFilter = [
                  "@system-service"
                  # deno (yt-dlp's JavaScript runtime) calls pkey_alloc while
                  # V8 starts.
                  "@pkey"
                  "~@privileged"
                ];
                # A filtered call fails instead of killing the process.
                SystemCallErrorNumber = "EPERM";
                # A second layer under the worker's egress proxy, which already
                # refuses every non-public address and the host's own interface
                # addresses for yt-dlp, its ffmpeg and the worker itself. Not
                # covered by either: the host's public IPv4 address behind NAT
                # (hairpin NAT), which only a firewall on the router can block.
                IPAddressDeny = [
                  "127.0.0.0/8"
                  "::1/128"
                  "169.254.0.0/16"
                  "fe80::/10"
                  "224.0.0.0/4"
                  "ff00::/8"
                  "0.0.0.0/8"
                  "10.0.0.0/8"
                  "100.64.0.0/10"
                  "172.16.0.0/12"
                  "192.168.0.0/16"
                  "fc00::/7"
                  # NAT64: reaches the embedded IPv4 address.
                  "64:ff9b::/96"
                  # The other IPv6 blocks the worker refuses: local-use NAT64,
                  # 6to4, IPv4-compatible and SIIT addresses (all can reach a
                  # private IPv4 address), and IANA's not globally reachable
                  # blocks (discard, IETF assignments incl. Teredo,
                  # documentation, SRv6, site-local). The worker itself
                  # allows the few global ones inside 2001::/23; nothing
                  # needed for downloads lives there.
                  "64:ff9b:1::/48"
                  "2002::/16"
                  "::/96"
                  "::ffff:0:0:0/96"
                  "100::/64"
                  "2001::/23"
                  "2001:db8::/32"
                  "3fff::/20"
                  "5f00::/16"
                  "fec0::/10"
                  # IPv4 ranges that are not globally reachable either: IETF
                  # protocol assignments, documentation, benchmarking, reserved.
                  "192.0.0.0/24"
                  "192.0.2.0/24"
                  "198.18.0.0/15"
                  "198.51.100.0/24"
                  "203.0.113.0/24"
                  "240.0.0.0/4"
                ];
                # 127.0.0.1: yt-dlp reaches the worker's egress proxy there (the
                # filter applies to loopback traffic as well). Like the resolver
                # entries this opens every port, so a process in the service
                # that bypassed the proxy could reach services bound to
                # 127.0.0.1; yt-dlp and ffmpeg are only given the proxy.
                IPAddressAllow = [ "127.0.0.1/32" ] ++ cfg.allowedAddresses;
              };
            };
          };
        };
    };
}
