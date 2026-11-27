{
  lib,
  stdenv,
  rustPlatform,
  buildEnv,
  symlinkJoin,
  makeWrapper,
  typst,
}:

let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
  pname = cargoToml.package.name;
  version = cargoToml.workspace.package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../crates
      ../TOLA-SKILL.md
    ];
  };

  # Wrap `tola` with nixpkgs Typst packages for offline Nix builds without vendoring.
  # Sites that prefer self-contained offline builds can use `tola vendor` + `tola build --pure`.
  withTypstPackages =
    base: selectPackages:
    let
      selected = selectPackages typst.packages;
      cache = buildEnv {
        name = "${pname}-typst-package-cache";
        paths = lib.concatMap (pkg: [ pkg ] ++ pkg.propagatedBuildInputs) selected;
        pathsToLink = [ "/lib/typst-packages" ];
        postBuild = ''
          mkdir -p "$out/lib/typst/packages"
          if [ -d "$out/lib/typst-packages" ]; then
            mv "$out/lib/typst-packages" "$out/lib/typst/packages/preview"
          fi
        '';
      };
    in
    symlinkJoin {
      name = "${base.name}-with-typst-packages";
      paths = [ base ];
      nativeBuildInputs = [ makeWrapper ];
      postBuild = ''
        wrapProgram "$out/bin/${pname}" \
          --set TYPST_PACKAGE_CACHE_PATH "${cache}/lib/typst/packages"
      '';
      passthru.withPackages = withTypstPackages base;
    };
in
rustPlatform.buildRustPackage (finalAttrs: {
  inherit pname version src;

  cargoLock.lockFile = ../Cargo.lock;

  # nu-system depends on libproc on macOS, whose build script invokes bindgen.
  nativeBuildInputs = lib.optionals stdenv.hostPlatform.isDarwin [
    rustPlatform.bindgenHook
  ];

  # Compile zlib statically from libz-sys instead of probing system zlib.
  env.LIBZ_SYS_STATIC = "1";

  doCheck = false;

  passthru.withPackages = withTypstPackages finalAttrs.finalPackage;

  meta = {
    inherit (cargoToml.package) description homepage;
    license = lib.licenses.mit;
    mainProgram = pname;
  };
})
