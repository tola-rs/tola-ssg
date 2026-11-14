# Validate a set of release archives and record the files to publish.

use ./archive.nu [archive-members, check-archive, extract-archive, file-sha256]
use ./semver.nu [without-build-metadata]
use ./targets.nu [archive-name, license-archive-name, target, target-names]

# Release license filenames and their repository paths.
export const LICENSE_FILES = [
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

# Check the archive on any host, and run the executable when requested.
export def verify-binary-archive [archive: path, binary: string, version: string, run: bool] {
    check-archive $archive $binary
    let stage = (mktemp --directory)
    try {
        extract-archive $archive $stage $binary
        let executable = ($stage | path join $binary)
        if ($executable | path type) != 'file' {
            error make { msg: $"($archive | path basename): ($binary) must be a regular file" }
        }
        if $run {
            let report = (^$executable --version | complete)
            if $report.exit_code != 0 {
                error make { msg: $"($binary) --version failed \(($report.exit_code)\): ($report.stderr | str trim)" }
            }
            let reported = ($report.stdout | lines | first | default '' | str trim)
            if $reported != $"tola ($version)" {
                error make { msg: $"($binary) reports '($reported)', expected 'tola ($version)'" }
            }
        }
    } finally {
        rm --recursive --force $stage
    }
}

def archive-file [file: record]: nothing -> record {
    {
        name: ($file.name | path basename)
        path: $file.name
        size: ($file.size | into int)
        sha256: (file-sha256 $file.name)
    }
}

export def verify-release [directory: path, version: string, targets: list<string>, --run: string]: nothing -> record {
    if ($directory | path type) != 'dir' {
        error make { msg: $"not a release directory: ($directory)" }
    }
    let plain = (without-build-metadata $version)
    let expected = ($targets | each {|name|
        {name: (archive-name $name $plain), binary: (target $name | get binary), run: ($name == $run)}
    })
    let names = ($expected | get name)
    let license = (license-archive-name $plain)
    let files = (ls --all --full-paths $directory)
    let actual = ($files | each {|file| $file.name | path basename })
    let required = if (($targets | length) == (target-names | length)) { $names ++ [$license] } else { $names }
    let missing = ($required | where {|name| $name not-in $actual })
    let unexpected = ($actual | where {|name| $name not-in ($names ++ [$license 'checksums.txt']) })
    let irregular = ($files | where type != 'file' | each {|file| $file.name | path basename })
    if ($missing | is-not-empty) or ($unexpected | is-not-empty) or ($irregular | is-not-empty) {
        error make {
            msg: $"release set mismatch: missing ($missing | str join ', '); unexpected ($unexpected | str join ', '); non-files ($irregular | str join ', ')"
        }
    }

    let by_name = ($files | reduce --fold {} {|file, by_name|
        $by_name | insert ($file.name | path basename) $file
    })
    let license_file = ($by_name | get --optional $license)
    let licenses = if $license_file != null {
        let members = (archive-members $license_file.name)
        let absent = ($LICENSE_FILES | get name | where {|name| $name not-in $members })
        if ($absent | is-not-empty) {
            error make { msg: $"($license) lacks ($absent | str join ', ')" }
        }
        archive-file $license_file
    }
    let archives = ($expected | each {|entry|
        let file = ($by_name | get $entry.name)
        verify-binary-archive $file.name $entry.binary $version $entry.run
        archive-file $file
    })
    {archives: $archives, licenses: $licenses}
}

export def checksum-text [archives: list<record>]: nothing -> string {
    let contents = ($archives | sort-by name | each {|entry| $"($entry.sha256)  ($entry.name)" } | str join (char lf))
    $"($contents)\n"
}
