{
  description = "opys — file-based feature inventory CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # Provides pinned Rust toolchains so `msrv` can reproduce the CI floor build.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    let
      # Cargo.toml's [workspace.package].version is the single source of truth
      # for the version (scripts/sync-versions.sh fans it out to the other
      # manifests); read it here so the Nix package never drifts from the crate.
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

      # Build the opys binary from this checkout. Factored out of the per-system
      # outputs so the exact same derivation backs both `packages.opys` and the
      # `overlays.default` that downstream flakes pull in.
      mkOpys = pkgs:
        let
          # The end-to-end pipe test (`opys list … | opys close -`) shells out
          # to `sh`, which the build sandbox doesn't place on PATH. Provide one
          # (bash in POSIX mode) just for the check phase.
          shForTests = pkgs.runCommand "opys-sh-for-tests" { } ''
            mkdir -p "$out/bin"
            ln -s ${pkgs.bash}/bin/bash "$out/bin/sh"
          '';
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "opys";
          version = cargoToml.workspace.package.version;

          # Only the inputs the build actually reads, so unrelated edits (README,
          # the packaging manifests) don't invalidate it. skills/ is required
          # because src/templates.rs embeds skills/opys/agent-rule.md.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              ./opys
              ./opys-backend-markdown-local
              ./skills
            ];
          };

          cargoLock.lockFile = ./Cargo.lock;

          # The workspace root package is the `opys-engine` *library*; the `opys`
          # binary lives in the opys/ member. Build (and test) that member so the
          # package actually produces the `opys` binary — a plain workspace-root
          # build installs no binary.
          buildAndTestSubdir = "opys";

          nativeCheckInputs = [ shForTests ];

          meta = {
            description = cargoToml.package.description;
            homepage = cargoToml.package.repository;
            license = pkgs.lib.licenses.asl20;
            mainProgram = "opys";
          };
        };

      # System-independent overlay so a downstream flake can do
      #   nixpkgs.overlays = [ opys.overlays.default ];   # -> pkgs.opys
      overlay = final: _prev: { opys = mkOpys final; };
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        opys = mkOpys pkgs;

        # The pinned MSRV toolchain — Cargo.toml's rust-version (kept as x.y)
        # normalized to x.y.0. `msrv` runs the exact CI floor build locally so a
        # dependency raising the minimum is caught here, not on the first push.
        msrvToolchain = pkgs.rust-bin.stable."${cargoToml.package.rust-version}.0".minimal;
        msrv = pkgs.writeShellScriptBin "msrv" ''
          export PATH="${msrvToolchain}/bin:$PATH"
          export CARGO_TARGET_DIR="''${CARGO_TARGET_DIR:-target/msrv}"
          echo "MSRV check with $(rustc --version)"
          exec cargo build --workspace --all-targets "$@"
        '';

        devPackages = with pkgs; [
          cargo
          rustc
          clippy
          rustfmt
          rust-analyzer
          gcc
        ];

        refresh = pkgs.writeShellScriptBin "refresh" ''
          nix build .#packages.${system}.dev-profile --out-link .nix-profile
        '';

        # Sync every packaging manifest's version to the crate version
        # (Cargo.toml is the source of truth). Run `sync-versions` to rewrite,
        # `sync-versions --check` as a CI gate. Wraps scripts/sync-versions.sh.
        sync-versions = pkgs.writeShellApplication {
          name = "sync-versions";
          runtimeInputs = with pkgs; [ gnused gawk gnugrep ];
          # The script resolves the repo root from its own location, so this
          # works as long as it's run from inside a checkout.
          text = ''exec bash ./scripts/sync-versions.sh "$@"'';
        };
      in
      {
        # The opys CLI — `nix build`, `nix run`, and downstream `packages` refs.
        packages.default = opys;
        packages.opys = opys;

        packages.dev-profile = pkgs.buildEnv {
          name = "opys-dev-profile";
          paths = devPackages ++ [ refresh msrv ];
        };

        apps.default = flake-utils.lib.mkApp { drv = opys; };
        apps.opys = flake-utils.lib.mkApp { drv = opys; };
        apps.sync-versions = {
          type = "app";
          program = "${sync-versions}/bin/sync-versions";
        };

        devShells.default = pkgs.mkShell {
          packages = devPackages ++ [ refresh sync-versions msrv ];

          shellHook = ''
            refresh
            export PATH="$PWD/.nix-profile/bin:$PATH"
          '';
        };
      }) // {
      # System-independent output: the overlay other flakes consume.
      overlays.default = overlay;
    };
}
