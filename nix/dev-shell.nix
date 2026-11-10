{
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
in
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
      nushell
      nodejs_24
      nixfmt
      # The maintenance scripts archive, compress, and fetch release inputs.
      zstd
      gzip
      gnutar
      curl
      # Maintenance suites and release tooling drive git checkouts.
      git
    ]
    ++ nativeBuildTools
    ++ nativeDependencies pkgs;
  # This shell exports LD_DYLD_PATH; rustc and cc link without it.
  shellHook = ''
    unset LD_DYLD_PATH
  '';
}
