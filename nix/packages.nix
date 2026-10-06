{
  lib,
  pkgs,
  cargoToml,
  src,
  buildEnvironment,
}:
let
  inherit (cargoToml.package) name description homepage;
  inherit (cargoToml.workspace.package) version;
  inherit (buildEnvironment)
    rustPackages
    nativeBuildTools
    nativeDependencies
    cargoEnv
    ;

  withTypstPackages =
    base: selectPackages:
    let
      selected = selectPackages pkgs.typst.packages;
      paths = lib.concatMap (pkg: [ pkg ] ++ pkg.propagatedBuildInputs) selected;
      cache = pkgs.buildEnv {
        name = "${name}-typst-package-cache";
        inherit paths;
        pathsToLink = [ "/lib/typst-packages" ];
        postBuild = ''
          mkdir -p "$out/lib/typst/packages"
          mv "$out/lib/typst-packages" "$out/lib/typst/packages/preview"
        '';
      };
    in
    pkgs.symlinkJoin {
      name = "${base.name}-with-typst-packages";
      paths = [ base ];
      nativeBuildInputs = [ pkgs.makeWrapper ];
      postBuild = ''
        wrapProgram "$out/bin/${name}" \
          --set TYPST_PACKAGE_CACHE_PATH ${cache}/lib/typst/packages
      '';
      passthru.withPackages = select': withTypstPackages base select';
    };

  mkTola =
    targetPkgs:
    let
      darwinDylibCheck = lib.optionalString targetPkgs.stdenv.hostPlatform.isDarwin ''
        deps="$(${targetPkgs.stdenv.cc.targetPrefix}otool -L "$out/bin/${name}")"
        bad="$(printf '%s\n' "$deps" | awk '/\/nix\/store\/.*\.dylib/ { print }')"
        if [ -n "$bad" ]; then
          echo "Darwin binary depends on Nix store dylibs:" >&2
          echo "$bad" >&2
          exit 1
        fi
      '';

      # A fresh derivation per call: the typst package cache joins the binary, so a wrapper
      # cannot select a different set for an already-built output.
      withBody =
        body: select:
        withTypstPackages ((rustPackages targetPkgs).rustPlatform.buildRustPackage (
          body
          // {
            pname = name;
            inherit version src;
            cargoLock.lockFile = ../Cargo.lock;
            nativeBuildInputs = nativeBuildTools ++ nativeDependencies pkgs;
            buildInputs = nativeDependencies targetPkgs;
            env = cargoEnv targetPkgs;
            doCheck = false;
            meta = {
              inherit description homepage;
              license = lib.licenses.mit;
              mainProgram = name;
            };
          }
        )) select;
    in
    (rustPackages targetPkgs).rustPlatform.buildRustPackage {
      pname = name;
      inherit version src;
      cargoLock.lockFile = ../Cargo.lock;

      nativeBuildInputs = nativeBuildTools ++ nativeDependencies pkgs;
      buildInputs = nativeDependencies targetPkgs;
      env = cargoEnv targetPkgs;

      doCheck = false;
      postFixup = darwinDylibCheck;

      meta = {
        inherit description homepage;
        license = lib.licenses.mit;
        mainProgram = name;
      };

      passthru.withPackages = withBody {
        postFixup = darwinDylibCheck;
      };
    };
in
# Nix users install the native build, or the static variant on Linux. Portable release
# archives are built by the release workflow from ordinary Cargo targets.
{
  default = mkTola pkgs;
}
// lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
  static = mkTola pkgs.pkgsStatic;
}
