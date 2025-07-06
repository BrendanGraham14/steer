{
  description = "Conductor - AI-powered CLI assistant for software engineering";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    crane.url = "github:ipetkov/crane";
    rust-advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
      crane,
      rust-advisory-db,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable."1.88.0".default.override {
          extensions = [
            "rust-src"
            "rustfmt"
            "clippy"
          ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Filter source to include proto files and all necessary cargo files
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type:
            let
              baseName = builtins.baseNameOf path;
              relPath = pkgs.lib.removePrefix (toString ./. + "/") (toString path);
            in
            # Default cargo/rust filters
            (craneLib.filterCargoSources path type)
            ||
              # Proto files and directory
              (type == "regular" && pkgs.lib.hasSuffix ".proto" path)
            || (type == "directory" && baseName == "proto")
            ||
              # Prompts files (markdown files in prompts directory)
              (pkgs.lib.hasPrefix "prompts/" relPath)
            ||
              # Migrations directory (SQL files)
              (pkgs.lib.hasPrefix "migrations/" relPath)
            ||
              # Ensure crates subdirectories are included
              (pkgs.lib.hasPrefix "crates/" relPath);
        };

        # Common arguments for all crane builds
        commonArgs = {
          inherit src;
          strictDeps = true;

          nativeBuildInputs = with pkgs; [
            pkg-config
            protobuf
            cmake
          ];

          buildInputs =
            with pkgs;
            [
              openssl
              sqlite
            ]
            ++ pkgs.lib.optionals stdenv.isDarwin [
              darwin.apple_sdk.frameworks.CoreServices
              darwin.apple_sdk.frameworks.SystemConfiguration
              darwin.apple_sdk.frameworks.Security
              libiconv
            ]
            ++ pkgs.lib.optionals stdenv.isLinux [
              # Linux-specific dependencies
            ];
        };

        # Build dependencies only (for better caching)
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Function to build a specific crate
        mkCrateCrane =
          {
            name,
            cargoPackage ? name,
          }:
          craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              pname = name;
              cargoExtraArgs = "-p ${cargoPackage}";
            }
          );

        devTools = with pkgs; [
          cargo-watch
          cargo-edit
          cargo-outdated
          cargo-audit
          cargo-nextest
          just
          bacon
          nushell
          # For development
          rust-analyzer
          # For MCP testing
          python3
          nodejs
        ];

      in
      {
        packages = {
          default = self.packages.${system}.conductor-cli;

          conductor-cli = mkCrateCrane {
            name = "conductor-cli";
          };

          conductor-remote-workspace = mkCrateCrane {
            name = "conductor-remote-workspace";
          };

          # Build all crates at once (useful for CI)
          conductor-workspace = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              pname = "conductor-workspace";
            }
          );
        };

        devShells = {
          default = pkgs.mkShell {
            inherit (commonArgs) buildInputs;
            nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ rustToolchain ] ++ devTools;

            shellHook = ''
              # This shellHook is executed only in interactive shells
              if [ -n "$PS1" ]; then
                # Source project-specific shell configuration if it exists
                [ -f ".conductor-shell.nix" ] && source .conductor-shell.nix
                
                echo ""
                echo "Welcome to Conductor development shell!"
                echo "Run 'just' to see available tasks."
              fi
            '';

            # Set up environment variables
            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
            RUST_BACKTRACE = 1;

            # For OpenSSL - append to existing PKG_CONFIG_PATH if it exists
            PKG_CONFIG_PATH = pkgs.lib.makeSearchPathOutput "dev" "lib/pkgconfig" [ pkgs.openssl ];
          };

          # Minimal shell for CI
          ci = pkgs.mkShell {
            inherit (commonArgs) buildInputs;
            nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ rustToolchain ];

            shellHook = ''
              echo "CI environment ready"
            '';
          };
        };

        apps = {
          default = self.apps.${system}.conductor;

          conductor = {
            type = "app";
            program = "${self.packages.${system}.conductor-cli}/bin/conductor";
          };

          remote-workspace = {
            type = "app";
            program = "${self.packages.${system}.conductor-remote-workspace}/bin/conductor-remote-workspace";
          };
        };

        # CI checks using crane
        checks = {
          # Run clippy on the crate
          conductor-clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets -- -D warnings";
            }
          );

          # Check formatting
          conductor-fmt = craneLib.cargoFmt {
            src = craneLib.cleanCargoSource ./.;
          };

          # Run tests
          conductor-tests = craneLib.cargoTest (
            commonArgs
            // {
              inherit cargoArtifacts;
            }
          );

          # Audit dependencies
          conductor-audit = craneLib.cargoAudit {
            src = craneLib.cleanCargoSource ./.;
            advisory-db = rust-advisory-db;
            cargoAuditExtraArgs = "--ignore RUSTSEC-2023-0071";
          };

          # Check documentation
          conductor-doc = craneLib.cargoDoc (
            commonArgs
            // {
              inherit cargoArtifacts;
            }
          );

          # Build all packages to ensure they compile
          conductor-build = self.packages.${system}.conductor-workspace;
        };
      }
    );
}
