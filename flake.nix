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

      # The node's web UI bundle. `opys-server/ui/dist` is NOT committed: it
      # reaches crates.io consumers inside the published tarball (`include` in
      # opys-server/Cargo.toml) and every build-from-source produces it here.
      # So the Nix package is one of the builds that must run npm — which is
      # fine in a derivation, because the lockfile is pinned and npmDepsHash
      # makes the dependency fetch a fixed-output derivation rather than
      # arbitrary network access at build time.
      #
      # Bump npmDepsHash whenever package-lock.json changes:
      #   nix run nixpkgs#prefetch-npm-deps -- opys-server/ui/package-lock.json
      mkUi = pkgs: pkgs.buildNpmPackage {
        pname = "opys-ui";
        version = cargoToml.workspace.package.version;

        # Only what `vite build` reads. dist/ is deliberately absent — this
        # derivation is what creates it — and node_modules must never enter the
        # store, or every `npm ci` in a developer checkout would rehash this.
        src = pkgs.lib.fileset.toSource {
          root = ./opys-server/ui;
          fileset = pkgs.lib.fileset.unions [
            ./opys-server/ui/package.json
            ./opys-server/ui/package-lock.json
            ./opys-server/ui/index.html
            ./opys-server/ui/svelte.config.js
            ./opys-server/ui/vite.config.js
            ./opys-server/ui/src
          ];
        };

        npmDepsHash = "sha256-bpdX2bMIRP7UedeDVty81/wTBR6824tYaGRMIKIv6rw=";

        # The bundle is the whole output: $out *is* dist/, so the consumer can
        # `cp -r ${ui} …/ui/dist` without reaching through a subdirectory.
        installPhase = ''
          runHook preInstall
          cp -r dist "$out"
          runHook postInstall
        '';
      };

      # Build the opys binary from this checkout. Factored out of the per-system
      # outputs so the exact same derivation backs both `packages.opys` and the
      # `overlays.default` that downstream flakes pull in.
      mkOpys = pkgs:
        let
          ui = mkUi pkgs;
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
              # opys-server minus the whole web UI tree, which `mkUi` owns and
              # postPatch below plants as ui/dist. Excluding it keeps two
              # unrelated things out of this derivation's hash: a developer's
              # node_modules (tens of megabytes, rewritten by every `npm ci`)
              # and the UI sources, whose effect arrives via the `${ui}` store
              # path instead. Do not "tidy" this down to src/ + Cargo.toml:
              # `opys web` links opys-server, so the sandbox compiles it —
              # build.rs and all — and needs the bundle to be there.
              (pkgs.lib.fileset.difference ./opys-server ./opys-server/ui)
              ./skills
            ];
          };

          cargoLock.lockFile = ./Cargo.lock;

          # Plant the separately-built bundle where opys-server/build.rs expects
          # it. The `web` feature is on by default, so this must happen for the
          # ordinary build: without it the crate fails to compile rather than
          # silently producing a node that serves a blank page.
          postPatch = ''
            mkdir -p opys-server/ui
            cp -r ${ui} opys-server/ui/dist
            chmod -R u+w opys-server/ui/dist
          '';

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
          # The web UI's build toolchain (ADR-0086). The *crate build* still
          # never runs Node — `cargo install`, docs.rs and the nix sandbox all
          # have to work with no Node and no network, and they get the bundle
          # prebuilt (from the crate tarball, or from `mkUi` above). What changed
          # is that opys-server/ui/dist is no longer committed, so building from
          # a checkout needs `ui-build` first — or `--no-default-features`, which
          # drops the `web-ui` feature and needs no Node at all.
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

        # Build the node's web UI bundle, opys-server/ui/dist (ADR-0086). It is
        # generated, not committed, so this runs after editing opys-server/ui/src
        # *and* on a fresh checkout before the first cargo build. Wraps
        # scripts/ui-build.sh.
        #
        # This is the only place Node is invoked interactively. The crate build
        # still never runs it; CI's cargo jobs now call this script first — see
        # devPackages.
        ui-build = pkgs.writeShellApplication {
          name = "ui-build";
          # No git, and no diffutils since ADR-0086 removed the drift gate. The
          # GNU tools because the script's audit uses `find -printf` and
          # `grep -r`, which BSD spells differently.
          runtimeInputs = with pkgs; [ nodejs_22 gnugrep findutils gawk coreutils ];
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
        # The web UI bundle on its own. `packages.opys` plants this into
        # opys-server/ui/dist, but exposing it separately makes the npm half
        # buildable — and npmDepsHash verifiable — without the Rust half.
        packages.opys-ui = mkUi pkgs;

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
