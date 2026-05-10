{
  description = "Static site generator for typst-based blog";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    flake-parts.url = "github:hercules-ci/flake-parts";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ flake-parts, nixpkgs, rust-overlay, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      perSystem = { lib, system, self', ... }:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };

          cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          packageName = cargoToml.package.name; # or "tola"
          packageVersion = cargoToml.package.version; # or "0.7.0"
          packageDescription = cargoToml.package.description;
          commonNativeBuildInputs = [ pkgs.nasm pkgs.perl pkgs.pkg-config ];
          darwinBuildInputs = lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconvReal ];
          nativeBuildInputs = commonNativeBuildInputs ++ darwinBuildInputs;
          libPath = lib.optionalString pkgs.stdenv.isDarwin
            (lib.makeLibraryPath [ pkgs.libiconvReal ]);

          typstPackageCache = selectPackages:
            let
              selectedPackages = selectPackages pkgs.typst.packages;
              packagePaths = lib.concatMap (
                pkg: [ pkg ] ++ pkg.propagatedBuildInputs
              ) selectedPackages;
            in
            pkgs.buildEnv {
              name = "${packageName}-typst-package-cache";
              paths = packagePaths;
              pathsToLink = [ "/lib/typst-packages" ];
              postBuild = ''
                export TYPST_LIB_DIR="$out/lib/typst/packages"
                mkdir -p "$out/lib/typst-packages"
                mkdir -p "$TYPST_LIB_DIR"
                mv "$out/lib/typst-packages" "$TYPST_LIB_DIR/preview"
              '';
            };

          wrapWithTypstPackages = basePackage: selectPackages:
            let
              packageCache = typstPackageCache selectPackages;
            in
            pkgs.symlinkJoin {
              name = "${basePackage.name}-with-typst-packages";
              paths = [ basePackage ];
              nativeBuildInputs = [ pkgs.makeWrapper ];
              postBuild = ''
                wrapProgram $out/bin/${packageName} \
                  --set TYPST_PACKAGE_CACHE_PATH ${packageCache}/lib/typst/packages
              '';
              passthru = {
                withPackages = selectPackages': wrapWithTypstPackages basePackage selectPackages';
              };
            };

          mkBaseTolaPackage = targetPkgs:
            targetPkgs.rustPlatform.buildRustPackage {
              pname = packageName;
              version = packageVersion;

              src = ./.;
              cargoLock.lockFile = ./Cargo.lock;

              inherit nativeBuildInputs;
              buildInputs = [ targetPkgs.openssl ];
              LIBRARY_PATH = libPath;

              doCheck = false;
              enableParallelBuilding = true;
              strictDeps = true;

              meta = {
                description = packageDescription;
                homepage = "https://github.com/tola-rs/tola-ssg";
                license = lib.licenses.mit;
                mainProgram = packageName;
              };
            };

          mkTolaPackageWithPackages = targetPkgs:
            let
              basePackage = mkBaseTolaPackage targetPkgs;
            in
            basePackage.overrideAttrs (_: {
              passthru = (basePackage.passthru or { }) // {
                withPackages = selectPackages: wrapWithTypstPackages basePackage selectPackages;
              };
            });

          crossTargets = {
            x86_64-linux = pkgs.pkgsCross.gnu64;
            x86_64-linux-static = pkgs.pkgsCross.gnu64.pkgsStatic;

            aarch64-linux = pkgs.pkgsCross.aarch64-multiplatform;
            aarch64-linux-static = pkgs.pkgsCross.aarch64-multiplatform.pkgsStatic;

            x86_64-windows = pkgs.pkgsCross.mingwW64;
            aarch64-darwin = pkgs.pkgsCross.aarch64-darwin;
          };

          packages = {
            default = mkTolaPackageWithPackages pkgs;
            static = mkTolaPackageWithPackages pkgs.pkgsStatic;
          } // lib.mapAttrs (_: targetPkgs: mkTolaPackageWithPackages targetPkgs) crossTargets;
        in
        {
          inherit packages;

          apps.default = {
            type = "app";
            program = "${self'.packages.default}/bin/tola";
          };

          checks.default = packages.default;

          devShells.default = pkgs.mkShell {
            packages = [ pkgs.rust-bin.stable.latest.default pkgs.openssl ] ++ nativeBuildInputs;
          };
        };
    };
}
