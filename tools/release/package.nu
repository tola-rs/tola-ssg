#!/usr/bin/env nu
# Build, verify, and checksum the release archives.

use ../lib/archive.nu *
use ../lib/repo.nu *
use ../lib/semver.nu *
use ../lib/targets.nu *

# License material the release's one archive carries: member name and repository path.
const LICENSE_FILES = [
    {name: 'LICENSE', path: 'LICENSE'}
    {name: 'LICENSE-1337-scheme', path: 'crates/tola-packages/licenses/LICENSE-1337-scheme'}
    {name: 'LICENSE-APACHE-2.0', path: 'crates/tola-lsp/LICENSE'}
    {name: 'LICENSE-CC-BY-SA-4.0', path: 'crates/tola-pronunciations/licenses/LICENSE-CC-BY-SA-4.0'}
    {name: 'LICENSE-EDRDG.html', path: 'crates/tola-pronunciations/licenses/EDRDG-LICENCE.html'}
    {name: 'LICENSE-UNICODE', path: 'crates/tola-pronunciations/licenses/LICENSE-UNICODE'}
    {name: 'LICENSE-a11y-syntax-highlighting', path: 'crates/tola-packages/licenses/LICENSE-a11y-syntax-highlighting'}
    {name: 'LICENSE-catppuccin-bat', path: 'crates/tola-packages/licenses/LICENSE-catppuccin-bat'}
    {name: 'LICENSE-coldark-bat', path: 'crates/tola-packages/licenses/LICENSE-coldark-bat'}
    {name: 'LICENSE-dracula-sublime', path: 'crates/tola-packages/licenses/LICENSE-dracula-sublime'}
    {name: 'LICENSE-gruvbox-tmtheme', path: 'crates/tola-packages/licenses/LICENSE-gruvbox-tmtheme'}
    {name: 'LICENSE-nord-plist', path: 'crates/tola-packages/licenses/LICENSE-nord-plist'}
    {name: 'LICENSE-onehalf', path: 'crates/tola-packages/licenses/LICENSE-onehalf'}
    {name: 'LICENSE-primer-github-textmate-theme', path: 'crates/tola-packages/licenses/LICENSE-primer-github-textmate-theme'}
    {name: 'LICENSE-solarized', path: 'crates/tola-packages/licenses/LICENSE-solarized'}
    {name: 'LICENSE-sublime-monokai-extended', path: 'crates/tola-packages/licenses/LICENSE-sublime-monokai-extended'}
    {name: 'LICENSE-sublime-snazzy', path: 'crates/tola-packages/licenses/LICENSE-sublime-snazzy'}
    {name: 'LICENSE-tokyonight-nvim', path: 'crates/tola-packages/licenses/LICENSE-tokyonight-nvim'}
    {name: 'LICENSE-twodark', path: 'crates/tola-packages/licenses/LICENSE-twodark'}
    {name: 'LICENSE-zenburn', path: 'crates/tola-packages/licenses/LICENSE-zenburn'}
    {name: 'NOTICE-TYPST-ASSETS', path: 'crates/tola-typst/NOTICE'}
    {name: 'PROVENANCE-TOLA-LSP.md', path: 'crates/tola-lsp/licenses/README.md'}
    {name: 'PROVENANCE-TOLA-PACKAGES.md', path: 'crates/tola-packages/licenses/README.md'}
    {name: 'PROVENANCE-TOLA-PRONUNCIATIONS.md', path: 'crates/tola-pronunciations/licenses/README.md'}
    {name: 'PROVENANCE-TOLA-TYPST.md', path: 'crates/tola-typst/licenses/README.md'}
    {name: 'THIRD-PARTY-LICENSES.txt', path: 'THIRD-PARTY-LICENSES.txt'}
    {name: 'THIRD-PARTY-GRAMMARS.txt', path: 'crates/tola-highlight/THIRD-PARTY-GRAMMARS.txt'}
]

const USAGE = 'usage: nu tools/release/package.nu build [TARGET] --output DIR
       nu tools/release/package.nu licenses --output DIR
       nu tools/release/package.nu verify DIR [--all]
       nu tools/release/package.nu checksums DIR [--all]'

# The repository root, two directories above this script.
def repository-root []: nothing -> path {
    $env.FILE_PWD | path expand | path dirname | path dirname
}

# Extract one target archive, run its binary when this host can, and hash the archive.
def verify-archive [archive: path, target: record, version: string, native: bool]: nothing -> string {
    let stage = (mktemp --directory)
    try {
        extract-archive $archive $stage $target
        if $native {
            let binary = ($stage | path join ($target | get binary))
            let report = (^$binary --version | complete)
            if $report.exit_code != 0 {
                error make { msg: $"($binary | path basename) --version failed \(($report.exit_code)\): ($report.stderr | str trim)" }
            }
            let reported = ($report.stdout | lines | get -o 0 | default '' | str trim)
            if $reported != $"tola ($version)" {
                error make { msg: $"($binary | path basename) reports '($reported)', expected 'tola ($version)'" }
            }
        }
        file-sha256 $archive
    } finally {
        rm --recursive --force $stage
    }
}

# Verify the selected release archives and return each archive's digest.
def verify-release [directory: path, version: string, targets: list<string>] {
    if ($directory | path type) != 'dir' {
        error make { msg: $"not a release directory: ($directory)" }
    }
    let plain = (without-build-metadata $version)
    let host = (host-target)
    let expected = (
        $targets | each {|name|
            {name: (archive-name $name $plain), target: (target $name), native: ($name == $host)}
        }
    )
    let licenses = (license-archive-name $plain)
    let license_archive = ($directory | path join $licenses)
    let license_required = ($license_archive | path exists) or (($targets | length) == (target-names | length))
    let names = ($expected | get name)
    let actual = (ls $directory | each {|entry| $entry.name | path basename })
    let required = if $license_required { ($names ++ [$licenses]) } else { $names }
    let missing = ($required | where {|name| $name not-in $actual })
    let unexpected = (
        $actual | where {|name|
            $name != 'checksums.txt' and $name != $licenses and $name not-in $names
        }
    )
    if ($missing | is-not-empty) or ($unexpected | is-not-empty) {
        error make {
            msg: $"release set mismatch: missing ($missing | str join ', '); unexpected ($unexpected | str join ', ')"
        }
    }
    if $license_required {
        let members = (archive-members $license_archive)
        let absent = ($LICENSE_FILES | get name | where {|name| $name not-in $members })
        if ($absent | is-not-empty) {
            error make { msg: $"($licenses) lacks ($absent | str join ', ')" }
        }
    }
    $expected | each {|entry|
        let archive = ($directory | path join $entry.name)
        {name: $entry.name, digest: (verify-archive $archive $entry.target $version $entry.native)}
    }
}

# Build one target, package and verify its archive, then place it.
def "main build" [target?: string, --output: directory = 'release'] {
    let root = (repository-root)
    let output = ($output | path expand)
    let name = ($target | default (host-target))
    let specification = (target $name)
    let version = (workspace-version $root)
    let plain = (without-build-metadata $version)
    let archive = ($output | path join (archive-name $name $plain))
    let binary = ($root | path join 'target' $name 'release' ($specification | get binary))
    let environment = if ($name | str ends-with '-musl') {
        # A Linux archive is the static one; a dynamic input fails the link here.
        {OPENSSL_STATIC: '1', RUSTFLAGS: '-C target-feature=+crt-static'}
    } else {
        {}
    }
    print $"Building ($name)"
    let build = (with-env $environment { ^cargo build --release --locked -p tola --target $name | complete })
    if $build.exit_code != 0 {
        error make { msg: $"cargo build failed \(($build.exit_code)\): ($build.stderr | str trim)" }
    }
    if not ($output | path exists) { mkdir $output }
    let stage = (mktemp --directory --tmpdir-path $output '.build-XXXXXXXX')
    try {
        let staged = ($stage | path join (archive-name $name $plain))
        create-archive $binary $staged $specification
        verify-archive $staged $specification $version ($name == (host-target)) | ignore
        mv --force $staged $archive
    } finally {
        rm --recursive --force $stage
    }
    print $"Wrote ($archive)"
}

# Place the release's license material in one archive.
def "main licenses" [--output: directory = 'release'] {
    let root = (repository-root)
    let output = ($output | path expand)
    let version = (without-build-metadata (workspace-version $root))
    let archive = ($output | path join (license-archive-name $version))
    if not ($output | path exists) { mkdir $output }
    let stage = (mktemp --directory)
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
        create-tar-gz $archive $members ($LICENSE_FILES | get name)
    } finally {
        rm --recursive --force $stage
    }
    print $"Wrote ($archive)"
}

# Verify the archives, and checksums.txt when the directory carries one.
def "main verify" [directory: directory, --all] {
    let path = ($directory | path expand)
    let targets = if $all { target-names } else { [ (host-target) ] }
    let digests = (verify-release $path (workspace-version (repository-root)) $targets)
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
        let expected = ($digests | each {|entry| {name: $entry.name, digest: ($entry.digest | str lowercase)} } | sort-by name)
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
    let path = ($directory | path expand)
    let targets = if $all { target-names } else { [ (host-target) ] }
    let digests = (verify-release $path (workspace-version (repository-root)) $targets)
    let contents = (
        $digests
        | sort-by name
        | each {|entry| $"($entry.digest)  ($entry.name)" }
        | str join "\n"
    )
    $"($contents)\n" | save --force ($path | path join 'checksums.txt')
    print $"Wrote ($path | path join 'checksums.txt')"
}

def main [] {
    print $USAGE
}
