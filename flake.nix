{
  description = "Static site generator for typst-based blog";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      perSystem = { lib, system, ... }:
        let
          pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ inputs.rust-overlay.overlays.default ];
          };

          rustToolchain = pkgs.rust-bin.stable.latest.minimal;

          mkPackage = targetPkgs:
            targetPkgs.rustPlatform.buildRustPackage {
              pname = "tola";
              version = "0.6.0";

              src = ./.;
              cargoLock.lockFile = ./Cargo.lock;

              nativeBuildInputs = with pkgs; [ nasm libiconvReal perl pkg-config ];
              buildInputs = [ targetPkgs.openssl ];
              env.LIBRARY_PATH = lib.makeLibraryPath [ pkgs.libiconvReal ];

              doCheck = false;
              enableParallelBuilding = true;

              meta = {
                description = "Static site generator for typst-based blog";
                homepage = "https://github.com/kawayww/tola-ssg";
                license = lib.licenses.mit;
              };
            };
        in
        {
          packages = {
            default = mkPackage pkgs;
            static = mkPackage pkgs.pkgsStatic;

            x86_64-linux = mkPackage pkgs.pkgsCross.gnu64;
            x86_64-linux-static = mkPackage pkgs.pkgsCross.gnu64.pkgsStatic;

            aarch64-linux = mkPackage pkgs.pkgsCross.aarch64-multiplatform;
            aarch64-linux-static = mkPackage pkgs.pkgsCross.aarch64-multiplatform.pkgsStatic;

            x86_64-windows = mkPackage pkgs.pkgsCross.mingwW64;
            aarch64-darwin = mkPackage pkgs.pkgsCross.aarch64-darwin;
          };
        };
    };
}
