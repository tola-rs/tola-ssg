{
  description = "Typst-based static site generator";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f system nixpkgs.legacyPackages.${system});
      # nixos-26.05 defaults to Rust 1.95; fall back once nixpkgs moves past 1.97.
      rustFor = pkgs: pkgs.rustPackages_1_97 or pkgs.rustPackages;
    in
    {
      packages = forAllSystems (
        _system: pkgs:
        {
          default = pkgs.callPackage ./nix/package.nix {
            inherit (rustFor pkgs) rustPlatform;
          };
        }
        // lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          static = pkgs.pkgsStatic.callPackage ./nix/package.nix {
            inherit (rustFor pkgs.pkgsStatic) rustPlatform;
            inherit (pkgs)
              buildEnv
              symlinkJoin
              makeWrapper
              typst
              ;
          };
        }
      );

      apps = forAllSystems (
        system: _pkgs:
        let
          tola = self.packages.${system}.default;
        in
        {
          default = {
            type = "app";
            program = lib.getExe tola;
            meta.description = tola.meta.description;
          };
        }
      );

      devShells = forAllSystems (
        _system: pkgs:
        let
          rust = rustFor pkgs;
        in
        {
          default = pkgs.mkShell {
            nativeBuildInputs = lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
              rust.rustPlatform.bindgenHook
            ];

            packages = with pkgs; [
              rust.rustc
              rust.cargo
              rust.clippy
              rust.rustfmt
              cargo-deny
              cargo-about
              just
              actionlint
              shellcheck
              nushell
              nodejs_24
              nixfmt
              zstd
              gzip
              gnutar
              curl
              git
            ];

            env.LIBZ_SYS_STATIC = "1";
          };
        }
      );

      formatter = forAllSystems (_system: pkgs: pkgs.nixfmt);
    };
}
