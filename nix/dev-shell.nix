{
  lib,
  pkgs,
  buildEnvironment,
}:
let
  inherit (buildEnvironment)
    rust
    cargoEnv
    nativeBuildTools
    nativeDependencies
    ;
  denoVersion = builtins.head (
    lib.concatMap (
      line:
      let
        version = builtins.match "deno[[:blank:]]+([^[:blank:]]+)[[:blank:]]*" line;
      in
      lib.optionals (version != null) version
    ) (lib.splitString "\n" (builtins.readFile ../.tool-versions))
  );
in
assert lib.assertMsg (
  pkgs.deno.version == denoVersion
) "The Nix development shell's Deno version must match .tool-versions";
pkgs.mkShell {
  env = cargoEnv pkgs;
  packages =
    with pkgs;
    [
      rust.rustc
      rust.cargo
      rust.clippy
      rust.rustfmt
      just
      actionlint
      deno
      nixfmt
      # Maintenance suites and release tooling drive git checkouts.
      git
    ]
    ++ nativeBuildTools
    ++ nativeDependencies pkgs;
  # Deno refuses to pass a DYLD_* variable to a child of a scoped `--allow-run`, and
  # this shell exports LD_DYLD_PATH; rustc and cc link without it.
  shellHook = ''
    unset LD_DYLD_PATH
  '';
}
