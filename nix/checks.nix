{
  lib,
  pkgs,
  packages,
  checkSource,
  name,
  rust,
}:
let
  fmtCheck =
    pkgs.runCommand "${name}-fmt-check"
      {
        nativeBuildInputs = [
          rust.cargo
          rust.rustc
          rust.rustfmt
          pkgs.just
        ];
        src = checkSource;
      }
      ''
        cp -R "$src" source
        chmod -R u+w source
        cd source
        just fmt-check
        touch "$out"
      '';

  cargoCheck =
    recipe:
    packages.default.overrideAttrs (previous: {
      pname = "${name}-${builtins.replaceStrings [ "::" ] [ "-" ] recipe}";
      src = checkSource;
      nativeBuildInputs =
        previous.nativeBuildInputs ++ [ pkgs.just ] ++ lib.optional (recipe == "clippy") rust.clippy;
      buildPhase = ''
        runHook preBuild
        just ${recipe}
        runHook postBuild
      '';
      installPhase = "mkdir -p $out";
      postFixup = "";
    });
in
{
  default = packages.default;
  fmt = fmtCheck;
  test = cargoCheck "test";
  clippy = cargoCheck "clippy";
  docs = cargoCheck "docs";
}
