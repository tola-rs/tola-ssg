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
          buildTools = [ pkgs.nasm pkgs.perl pkgs.pkg-config ];

          needsOpenSSL = packageSet:
            let
              platform = packageSet.stdenv.hostPlatform;
            in
            !(platform.isDarwin || platform.isWindows);

          buildPlatformInputs =
            lib.optionals (needsOpenSSL pkgs) [ pkgs.openssl ];

          hostPlatformInputs = packageSet:
            lib.optionals (needsOpenSSL packageSet) [ packageSet.openssl ];

          darwinLinkEnv = targetPkgs:
            let
              platform = targetPkgs.stdenv.hostPlatform;
              ccSuffix = builtins.replaceStrings [ "-" ] [ "_" ] platform.config;
            in
            lib.optionalAttrs platform.isDarwin {
              "NIX_LDFLAGS_${ccSuffix}" = "-dead_strip_dylibs";
            };

          cargoEnv = targetPkgs:
            {
              OPENSSL_NO_VENDOR = true;
            }
            // darwinLinkEnv targetPkgs;

          darwinReleaseCheck = targetPkgs:
            lib.optionalString targetPkgs.stdenv.hostPlatform.isDarwin ''
              deps="$(${targetPkgs.stdenv.cc.targetPrefix}otool -L "$out/bin/${packageName}")"
              badDeps="$(printf '%s\n' "$deps" | awk '/\/nix\/store\/.*\.dylib/ { print }')"
              if [ -n "$badDeps" ]; then
                echo "Darwin binary depends on Nix store dylibs:" >&2
                echo "$badDeps" >&2
                exit 1
              fi
            '';

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

              nativeBuildInputs = buildTools ++ buildPlatformInputs;
              buildInputs = hostPlatformInputs targetPkgs;
              env = cargoEnv targetPkgs;

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

          mkReleaseTolaPackage = targetPkgs:
            (mkTolaPackageWithPackages targetPkgs).overrideAttrs (old: {
              postFixup = lib.concatStringsSep "\n" [
                (old.postFixup or "")
                (darwinReleaseCheck targetPkgs)
              ];
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
            aarch64-darwin-release = mkReleaseTolaPackage pkgs.pkgsCross.aarch64-darwin;
          }
          // lib.mapAttrs (_: targetPkgs: mkTolaPackageWithPackages targetPkgs) crossTargets;
        in
        {
          inherit packages;

          apps.default = {
            type = "app";
            program = "${self'.packages.default}/bin/tola";
            meta.description = packageDescription;
          };

          checks.default = packages.default;

          devShells.default = pkgs.mkShell {
            packages = [ pkgs.rust-bin.stable.latest.default ] ++ buildTools;
            buildInputs = hostPlatformInputs pkgs;
            env = cargoEnv pkgs;
          };
        };
    };
}
