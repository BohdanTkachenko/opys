{
  description = "opys — file-based feature inventory CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

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
        apps.sync-versions = {
          type = "app";
          program = "${sync-versions}/bin/sync-versions";
        };

        packages.dev-profile = pkgs.buildEnv {
          name = "opys-dev-profile";
          paths = devPackages ++ [ refresh ];
        };

        devShells.default = pkgs.mkShell {
          packages = devPackages ++ [ refresh sync-versions ];

          shellHook = ''
            refresh
            export PATH="$PWD/.nix-profile/bin:$PATH"
          '';
        };
      });
}
