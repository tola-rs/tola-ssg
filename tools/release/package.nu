#!/usr/bin/env nu
# Build, verify, and checksum the release archives.

use ../lib/archive.nu [create-archive, create-tar-gz]
use ../lib/release-archives.nu [LICENSE_FILES, verify-binary-archive, verify-release, checksum-text]
use ../lib/repo.nu *
use ../lib/semver.nu *
use ../lib/targets.nu *

const USAGE = 'usage: nu tools/release/package.nu build [TARGET] --output DIR
       nu tools/release/package.nu licenses --output DIR
       nu tools/release/package.nu verify DIR [--all]
       nu tools/release/package.nu checksums DIR [--all]'

# The repository root, two directories above this script.
def repository-root []: nothing -> path {
    $env.FILE_PWD | path expand | path dirname | path dirname
}

def require-archive-destination [archive: path] {
    let type = ($archive | path type | default '')
    if $type not-in ['' 'file'] {
        error make { msg: $"($archive | path basename): destination must be a regular file" }
    }
}

# Build one target, package and verify its archive, then place it.
def "main build" [target?: string, --output: directory = 'release'] {
    let root = (repository-root)
    let output = ($output | path expand --no-symlink)
    let name = if $target == null { host-target } else { $target }
    let specification = (target $name)
    let version = (workspace-version $root)
    let plain = (without-build-metadata $version)
    let archive = ($output | path join (archive-name $name $plain))
    let environment = if ($name | str ends-with '-musl') {
        # A Linux archive is the static one; a dynamic input fails the link here.
        {OPENSSL_STATIC: '1', RUSTFLAGS: '-C target-feature=+crt-static'}
    } else {
        {}
    }
    print $"Building ($name)"
    cd $root
    let manifest = ($root | path join $MANIFEST)
    let build = (with-env $environment {
        ^cargo build --release --locked -p tola --bin tola --target $name --message-format=json-render-diagnostics | complete
    })
    if ($build.stderr | is-not-empty) { print --stderr --no-newline $build.stderr }
    if $build.exit_code != 0 {
        error make { msg: $"cargo build for ($name) failed \(($build.exit_code)\)" }
    }
    let binaries = ($build.stdout | from json --objects
        | where {|message|
            $message.reason == 'compiler-artifact' and ($message.manifest_path | path expand) == $manifest and $message.executable != null
        }
        | get executable
    )
    if ($binaries | length) != 1 {
        error make { msg: $"cargo build did not report exactly one tola executable for ($name)" }
    }
    let binary = ($binaries | first)
    if not ($output | path exists) { mkdir $output }
    if ($output | path type) != 'dir' { error make { msg: 'release output must be a directory' } }
    let stage = (mktemp --directory --tmpdir-path $output '.build-XXXXXXXX')
    try {
        let staged = ($stage | path join (archive-name $name $plain))
        create-archive $binary $staged $specification
        verify-binary-archive $staged $specification.binary $version (matches-host $specification)
        require-archive-destination $archive
        mv --force $staged $archive
    } finally {
        rm --recursive --force $stage
    }
    print $"Wrote ($archive)"
}

# Place the release's license material in one archive.
def "main licenses" [--output: directory = 'release'] {
    let root = (repository-root)
    let output = ($output | path expand --no-symlink)
    let version = (without-build-metadata (workspace-version $root))
    let archive = ($output | path join (license-archive-name $version))
    if not ($output | path exists) { mkdir $output }
    if ($output | path type) != 'dir' { error make { msg: 'release output must be a directory' } }
    let stage = (mktemp --directory --tmpdir-path $output '.licenses-XXXXXXXX')
    try {
        let members = ($stage | path join 'members')
        mkdir $members
        for entry in $LICENSE_FILES {
            let source = ($root | path join $entry.path)
            if not ($source | path exists) {
                error make { msg: $"missing ($entry.path); run 'just licenses' before packaging" }
            }
            cp $source ($members | path join $entry.name)
        }
        let staged = ($stage | path join (license-archive-name $version))
        create-tar-gz $staged $members ($LICENSE_FILES | get name)
        require-archive-destination $archive
        mv --force $staged $archive
    } finally {
        rm --recursive --force $stage
    }
    print $"Wrote ($archive)"
}

# Verify the archives, and checksums.txt when the directory carries one.
def "main verify" [directory: directory, --all] {
    let path = ($directory | path expand --no-symlink)
    let targets = if $all { target-names } else { [ (host-target) ] }
    let version = (workspace-version (repository-root))
    let run = if $all { '' } else { $targets | first }
    let release = (verify-release $path $version $targets --run $run)
    let checksums = ($path | path join 'checksums.txt')
    if ($checksums | path exists) {
        let entries = (
            open --raw $checksums
            | lines
            | where {|line| ($line | str trim) != '' }
            | each {|line|
                let parsed = ($line | parse --regex '^(?<digest>[0-9a-fA-F]{64}) [ *](?<name>[A-Za-z0-9._-]+)$')
                if ($parsed | is-empty) {
                    error make { msg: $"invalid checksums entry in checksums.txt: '($line)'" }
                }
                $parsed | first
            }
        )
        let duplicated = ($entries | each {|entry| $entry.name} | sort | uniq -d)
        let recorded = ($entries | each {|entry| {name: $entry.name, digest: ($entry.digest | str lowercase)} } | sort-by name)
        let expected = ($release.archives | select name sha256 | rename name digest | sort-by name)
        if ($duplicated | is-not-empty) or ($recorded != $expected) {
            let missing = (($expected | get name) | where {|name| $name not-in ($recorded | each {|entry| $entry.name}) })
            let unexpected = (($recorded | each {|entry| $entry.name}) | where {|name| $name not-in ($expected | get name) })
            error make {
                msg: $"checksums.txt disagrees with the archives: missing ($missing | str join ', '); unexpected ($unexpected | str join ', ')"
            }
        }
    }
    print $"Verified ($path)"
}

# Replace checksums.txt with the digest of every verified archive, sorted by name.
def "main checksums" [directory: directory, --all] {
    let path = ($directory | path expand --no-symlink)
    let targets = if $all { target-names } else { [ (host-target) ] }
    let version = (workspace-version (repository-root))
    let run = if $all { '' } else { $targets | first }
    let release = (verify-release $path $version $targets --run $run)
    checksum-text $release.archives | save --force ($path | path join 'checksums.txt')
    print $"Wrote ($path | path join 'checksums.txt')"
}

def main [] {
    print $USAGE
}
