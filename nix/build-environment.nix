{
  lib,
  pkgs,
  rustVersion,
}:
let
  # nixpkgs carries one package set per Rust release and does not always have the exact
  # rust-version, so the smallest set new enough to compile the workspace is the one to use.
  rustPackages =
    targetPkgs:
    let
      versionOf = name: lib.replaceStrings [ "_" ] [ "." ] (lib.removePrefix "rustPackages_" name);
      packaged = builtins.filter (lib.hasPrefix "rustPackages_") (builtins.attrNames targetPkgs);
      suitable = builtins.filter (name: lib.versionAtLeast (versionOf name) rustVersion) packaged;
      ordered = builtins.sort (left: right: lib.versionOlder (versionOf left) (versionOf right)) suitable;
    in
    assert lib.assertMsg (ordered != [ ])
      "nixpkgs carries no Rust ${rustVersion} or newer package set; lower rust-version or update the nixpkgs input";
    targetPkgs.${builtins.head ordered};
in
{
  inherit rustPackages;
  rust = rustPackages pkgs;

  nativeBuildTools = with pkgs; [
    perl
    pkg-config
  ];

  # tola reaches TLS through rustls, so openssl leaves Darwin alone; the assembler is absent
  # because nothing in the workspace builds one.
  nativeDependencies =
    targetPkgs: lib.optionals targetPkgs.stdenv.hostPlatform.isLinux [ targetPkgs.openssl ];

  cargoEnv =
    targetPkgs:
    lib.optionalAttrs targetPkgs.stdenv.hostPlatform.isLinux {
      OPENSSL_NO_VENDOR = true;
    };
}
