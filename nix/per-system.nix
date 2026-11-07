{
  lib,
  pkgs,
  self',
  ...
}:
let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
  buildEnvironment = import ./build-environment.nix {
    inherit lib pkgs;
    rustVersion = cargoToml.workspace.package.rust-version;
  };

  sourceFiles = lib.fileset.unions [
    ../Cargo.toml
    ../Cargo.lock
    ../src
    ../crates
    ../TOLA-SKILL.md
  ];
  src = lib.fileset.toSource {
    root = ../.;
    fileset = sourceFiles;
  };
  packages = import ./packages.nix {
    inherit
      lib
      pkgs
      cargoToml
      src
      buildEnvironment
      ;
  };
in
{
  inherit packages;

  apps.default = {
    type = "app";
    program = lib.getExe self'.packages.default;
    meta.description = cargoToml.package.description;
  };

  devShells.default = import ./dev-shell.nix {
    inherit pkgs buildEnvironment;
  };
}