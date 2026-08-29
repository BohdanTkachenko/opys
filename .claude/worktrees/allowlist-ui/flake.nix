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
          # because src/templates.rs embeds skills/opys/agent-rule.md, and
          # opys-server/ because the `opys` binary links it for `opys web`
          # (ADR-0077) — it is compiled here, build.rs and all.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              ./opys
              ./opys-backend-markdown-local
              # opys-server minus the web UI's node_modules. A developer who has
              # run `ui-build` has tens of megabytes of npm packages sitting
              # there, and copying them into the store would rehash this
              # derivation on every `npm ci`. `maybeMissing`, because a clean
              # checkout has no such directory and `difference` against a
              # nonexistent path is an eval error.
              #
              # ui/dist stays *in* — it is committed, and build.rs embeds it
              # (ADR-0078). Do not "tidy" this down to src/ + Cargo.toml: `opys
              # web` links opys-server, so the sandbox compiles it and needs the
              # bundle.
              (pkgs.lib.fileset.difference ./opys-server
                (pkgs.lib.fileset.maybeMissing ./opys-server/ui/node_modules))
              ./skills
            ];
          };

          cargoLock.lockFile = ./Cargo.lock;

          # The workspace root package is the `opys-engine` *library*; the `opys`
          # binary lives in the opys/ member. Build (and test) that member so the
          # package actually produces the `opys` binary — a plain workspace-root
          # build installs no binary.
          buildAndTestSubdir = "opys";

          # git as well as sh: `renumber_keeps_a_relocated_base_document` and the
          # history tests build a real repo in a tempdir, and the sandbox has no
          # git on PATH either. Without it `nix build .#opys` fails in the check
          # phase — which is the path every `pkgs.opys` consumer takes.
          nativeCheckInputs = [ shForTests pkgs.git ];

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
          # The web UI's build toolchain (ADR-0078). Node lives here and nowhere
          # else: the crate build never runs it, because `cargo install`,
          # docs.rs, and the nix sandbox all have to work with no Node and no
          # network. `ui-build` regenerates opys-server/ui/dist, which is
          # committed and checked for drift in CI.
          nodejs_22
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

        # Rebuild the node's committed web UI bundle, opys-server/ui/dist
        # (ADR-0078). Run `ui-build` after editing opys-server/ui/src;
        # `ui-build --check` is the CI drift gate. Wraps scripts/ui-build.sh.
        #
        # This is the *only* place Node is invoked. Nothing in the crate build,
        # and nothing in CI's cargo jobs, may ever run it — see the comment on
        # devPackages.
        ui-build = pkgs.writeShellApplication {
          name = "ui-build";
          # No git: the drift gate compares the rebuilt bundle against the one
          # that was in the tree, which is a question about bytes, not about a
          # working copy (see the gate's comment in scripts/ui-build.sh).
          # diffutils answers it. The GNU tools because the script's audit uses
          # `find -printf` and `grep -r`, which BSD spells differently.
          runtimeInputs = with pkgs; [ nodejs_22 diffutils gnugrep findutils gawk coreutils ];
          # Find the checkout by walking up from the cwd, rather than running
          # `./scripts/ui-build.sh` (which needs the repo root as the cwd) or the
          # store copy of the script (which resolves the tree from its own
          # location, and would find /nix/store). The one command a UI
          # contributor is told to run has to work from opys-server/ui, which is
          # where the README puts them.
          text = ''
            dir=$PWD
            while [ ! -f "$dir/scripts/ui-build.sh" ]; do
              if [ "$dir" = / ] || [ -z "$dir" ]; then
                echo "ui-build: run this from inside an opys checkout" >&2
                exit 2
              fi
              dir=$(dirname "$dir")
            done
            exec bash "$dir/scripts/ui-build.sh" "$@"
          '';
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
        apps.ui-build = {
          type = "app";
          program = "${ui-build}/bin/ui-build";
        };

        devShells.default = pkgs.mkShell {
          # ui-build belongs to the shell, not to devPackages: devPackages feeds
          # packages.dev-profile, which `refresh` rebuilds, and a mkShell package
          # only needs the shell re-entered.
          packages = devPackages ++ [ refresh sync-versions msrv ui-build ];

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
