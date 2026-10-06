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
    ../.agents/skills/tola/SKILL.md
  ];
  src = lib.fileset.toSource {
    root = ../.;
    fileset = sourceFiles;
  };
  # `just check` feeds this tree to the workspace-wide formatters and linters, so it carries
  # every root each of them reads, with the build output and installed dependencies Deno
  # already excludes left out of the copy.
  checkSource = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      sourceFiles
      (lib.fileset.difference ../e2e (
        lib.fileset.unions (
          map lib.fileset.maybeMissing [
            ../e2e/playwright-report
            ../e2e/test-results
          ]
        )
      ))
      (lib.fileset.difference ../extensions (
        lib.fileset.unions (
          map lib.fileset.maybeMissing [
            ../extensions/vscode/dist
            ../extensions/vscode/node_modules
            ../extensions/vscode/.vscode-test
          ]
        )
      ))
      ../deno.jsonc
      ../deno.lock
      ../just
      ../justfile
      ../scripts
    ];
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

  checks = import ./checks.nix {
    inherit
      lib
      pkgs
      packages
      checkSource
      ;
    inherit (cargoToml.package) name;
    inherit (buildEnvironment) rust;
  };

  devShells.default = import ./dev-shell.nix {
    inherit lib pkgs buildEnvironment;
  };
}
