{
  description = "TuneMesh: a Syncthing-backed personal music player and its download worker";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/c5c4a43b0e8056328ec4529f735cabdb8f1942bb";

  outputs =
    { self, nixpkgs }:
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
        '';

        meta = {
          description = "Spotify-like desktop player for a Syncthing music library";
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

        postInstall = ''
          wrapProgram $out/bin/tunemesh-worker \
            --prefix PATH : ${
              lib.makeBinPath [
                pkgs.yt-dlp
                pkgs.ffmpeg
                pkgs.spotdl
              ]
            }
        '';

        meta = {
          description = "TuneMesh download worker (yt-dlp / spotDL)";
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
      };

      devShells.${system}.default = pkgs.mkShell {
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
          spotdl
        ];
        shellHook = ''
          export XDG_DATA_DIRS="${pkgs.adwaita-icon-theme}/share:${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}:$XDG_DATA_DIRS"
        '';
      };

      nixosModules.worker =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.tunemesh-worker;
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
              example = "/home/thomas/Music/TuneMesh";
              description = "Root of the TuneMesh library (the Syncthing folder).";
            };
            workDir = lib.mkOption {
              type = lib.types.str;
              default = "/var/cache/tunemesh-worker";
              description = ''
                Directory outside the library where downloads are assembled
                before they are moved into place. Created and owned by `user`.
              '';
            };
          };

          config = lib.mkIf cfg.enable {
            systemd.tmpfiles.settings."10-tunemesh-worker".${cfg.workDir}.d = {
              inherit (cfg) user;
              group = config.users.users.${cfg.user}.group;
              mode = "0750";
            };

            systemd.services.tunemesh-worker = {
              description = "TuneMesh download worker";
              wantedBy = [ "multi-user.target" ];
              wants = [ "network-online.target" ];
              after = [ "network-online.target" ];
              serviceConfig = {
                User = cfg.user;
                Group = config.users.users.${cfg.user}.group;
                ExecStart = "${lib.getExe cfg.package} --library ${lib.escapeShellArg cfg.libraryPath} --work-dir ${lib.escapeShellArg cfg.workDir}";
                Restart = "on-failure";
                RestartSec = 10;
                # Files written into the Syncthing folder stay group-readable
                # like the user's own files.
                UMask = "0022";
                Nice = 10;
                IOSchedulingClass = "idle";
              };
            };
          };
        };
    };
}
