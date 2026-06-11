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
      in
      {
        packages.dev-profile = pkgs.buildEnv {
          name = "opys-dev-profile";
          paths = devPackages ++ [ refresh ];
        };

        devShells.default = pkgs.mkShell {
          packages = devPackages ++ [ refresh ];

          shellHook = ''
            refresh
            export PATH="$PWD/.nix-profile/bin:$PATH"
          '';
        };
      });
}
