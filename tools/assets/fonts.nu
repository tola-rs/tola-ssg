#!/usr/bin/env nu
# Regenerate the fonts this repository carries from the pinned typst-assets release.
#
# The version and checksum come from Cargo.lock. The crate is downloaded, verified, and
# unpacked, then the container, NOTICE, and provenance are rewritten. Passing --check writes
# nothing: it decodes the committed container's zstd frame and compares the payload, so a
# frame that differs while the content does not is not a difference.

use ../lib/assets.nu *

const CARRIED = [
    'LibertinusSerif-Regular.otf'
    'LibertinusSerif-Bold.otf'
    'LibertinusSerif-Italic.otf'
    'LibertinusSerif-BoldItalic.otf'
    'LibertinusSerif-Semibold.otf'
    'LibertinusSerif-SemiboldItalic.otf'
    'NewCMMath-Bold.otf'
    'NewCMMath-Book.otf'
    'NewCMMath-Regular.otf'
    'NewCM10-Regular.otf'
    'NewCM10-Bold.otf'
    'NewCM10-Italic.otf'
    'NewCM10-BoldItalic.otf'
    'DejaVuSansMono-Bold.ttf'
    'DejaVuSansMono-BoldOblique.ttf'
    'DejaVuSansMono-Oblique.ttf'
    'DejaVuSansMono.ttf'
]

const CONTAINER = 'crates/tola-typst/assets/embedded-fonts.bin'
const NOTICE = 'crates/tola-typst/NOTICE'
const PROVENANCE = 'crates/tola-typst/licenses/README.md'
const CRATE = 'https://static.crates.io/crates/typst-assets'
const MAGIC = 'TOLAFNT1'

# The typst-assets version and checksum the workspace locks.
def pinned [lock: string] {
    let entry = (
        $lock
        | parse -r '\[\[package\]\]\nname = "typst-assets"\nversion = "(?<version>[^"]+)"\n[^\n]*\nchecksum = "(?<sha256>[0-9a-f]{64})"'
        | get -o 0
    )
    if $entry == null { error make { msg: 'Cargo.lock names no locked typst-assets release' } }
    { version: $entry.version, sha256: $entry.sha256 }
}

# The little-endian u32 spelling of a length or offset.
def le32 [value: int] {
    bytes build ($value mod 256) (($value // 256) mod 256) (($value // 65536) mod 256) (($value // 16777216) mod 256)
}

# An index naming each font's offset and length: the container before its frame.
def indexed [fonts: list] {
    let offsets = (
        0..<($fonts | length) | each {|row|
            if $row == 0 { 0 } else { ($fonts | first $row | get length | math sum) }
        }
    )
    let entries = (
        $fonts | enumerate | each {|entry|
            let font = $entry.item
            bytes build ($font.name | str length) ($font.name | into binary) (le32 ($offsets | get $entry.index)) (le32 $font.length)
        }
    )
    ([($MAGIC | into binary) (le32 ($fonts | length))] ++ $entries | bytes collect)
}

# One byte of a hex string.
def read-byte [hex: string, at: int] {
    let start = ($at * 2)
    let end = ($start + 2)
    $hex | str substring $start..<$end | into int --radix 16
}

# The little-endian u32 four bytes at `at` in a hex string.
def read-u32 [hex: string, at: int] {
    let bytes = (0..<4 | each {|index| read-byte $hex ($at + $index) })
    ($bytes | get 0) + 256 * ($bytes | get 1) + 65536 * ($bytes | get 2) + 16777216 * ($bytes | get 3)
}

# The byte where a container's zstd frame starts.
def frame-start [container: binary] {
    let hex = ($container | encode hex)
    if not ($hex | str starts-with ($MAGIC | into binary | encode hex)) {
        error make { msg: 'the embedded font container does not open with its magic' }
    }
    let count = (read-u32 $hex 8)
    mut at = 12
    for row in 0..<$count {
        $at = ($at + 9 + (read-byte $hex $at))
    }
    $at
}

# Unpack the carried fonts and the notice out of the release's crate.
def unpack [archive: string, version: string, work: string] {
    let prefix = $"typst-assets-($version)/"
    let members = ($CARRIED | each {|name| $"($prefix)files/fonts/($name)" } | append $"($prefix)NOTICE")
    let unpacked = (^tar -xzf $archive -C $work --strip-components 1 ...$members | complete)
    if $unpacked.exit_code != 0 {
        error make { msg: $"the typst-assets ($version) crate could not be unpacked" }
    }
}

# The provenance file: where the carried fonts come from, and what they hash to.
def provenance [version: string, checksum: string, fonts: list] {
    let lines = [
        '# Fonts'
        ''
        '`tola-typst` includes these fonts when its `embed-fonts` feature is enabled. They are'
        'redistributed unmodified from [typst-assets](https://github.com/typst/typst-assets)'
        $"($version), the release the workspace locks with checksum `($checksum)`. The crate's"
        "`NOTICE` is that release's notice."
        ''
        'The fonts are embedded compressed in `assets/embedded-fonts.bin`, behind an index naming each'
        'one; a build reads the file and never fetches it. Regenerate with `just tools::fonts`,'
        'which also fails when the embedded fonts stop matching this release.'
        ''
        '| File | Bytes | SHA-256 |'
        '| --- | ---: | --- |'
    ]
    let rows = ($fonts | each {|font| $"| ($font.name) | ($font.length) | ($font.sha256) |"})
    ($lines ++ $rows ++ [''] | str join (char newline))
}

# Whether a file already holds exactly this text.
def holds [path: string, text: string] {
    if not ($path | path exists) { return false }
    (open --raw $path) == $text
}

# The digest of the payload a committed container decodes to.
def decoded-hash [path: string, work: string] {
    if not ($path | path exists) { return null }
    let container = (open --raw $path)
    let frame = ($work | path join 'committed.zst')
    let bytes = ($container | bytes at (frame-start $container)..)
    $bytes | save --force $frame
    let payload = ($work | path join 'committed.bin')
    let decoded = (^zstd -d -q -f -o $payload $frame | complete)
    if $decoded.exit_code != 0 {
        error make { msg: 'the committed embedded font payload does not decode' }
    }
    open --raw $payload | hash sha256
}

def write-file [path: string, content: any, root: string] {
    $content | save --force $path
    print $"($path | path relative-to $root)  ($content | into binary | bytes length) bytes"
}

def main [--check] {
    let root = ($env.FILE_PWD | path dirname | path dirname)
    let release = (pinned (open --raw ($root | path join 'Cargo.lock')))
    let work = (mktemp --directory)
    try {
        let archive = ($work | path join 'typst-assets.crate')
        let url = $"($CRATE)/typst-assets-($release.version).crate"
        download $url $archive
        let observed = (open --raw $archive | hash sha256)
        if $observed != $release.sha256 {
            error make { msg: $"typst-assets ($release.version) hashes to ($observed), not the locked ($release.sha256)" }
        }
        unpack $archive $release.version $work

        let fonts = (
            $CARRIED | each {|name|
                let path = ($work | path join 'files' 'fonts' $name)
                if not ($path | path exists) {
                    error make { msg: $"typst-assets ($release.version) carries no ($name)" }
                }
                let bytes = (open --raw $path)
                { name: $name, bytes: $bytes, length: ($bytes | bytes length), sha256: ($bytes | hash sha256) }
            }
        )
        let payload = ($fonts | each {|font| $font.bytes } | bytes collect)
        let raw = ($work | path join 'payload.bin')
        $payload | save --force $raw
        let frame = ($work | path join 'payload.zst')
        compress-zstd $raw $frame
        let framed = ([(indexed $fonts) (open --raw $frame)] | bytes collect)

        let notice = (open --raw ($work | path join 'NOTICE'))
        let readme = (provenance $release.version $release.sha256 $fonts)
        let container = ($root | path join $CONTAINER)
        let notice_path = ($root | path join $NOTICE)
        let readme_path = ($root | path join $PROVENANCE)

        if $check {
            mut stale = []
            let decoded = (decoded-hash $container $work)
            if $decoded == null or $decoded != ($payload | hash sha256) {
                $stale = ($stale | append $CONTAINER)
            }
            if not (holds $notice_path $notice) { $stale = ($stale | append $NOTICE) }
            if not (holds $readme_path $readme) { $stale = ($stale | append $PROVENANCE) }
            if ($stale | is-not-empty) {
                error make { msg: $"the carried fonts are stale, rerun `just tools::fonts`: ($stale | str join ', ')" }
            }
            print $"the carried fonts are current with typst-assets ($release.version)"
        } else {
            write-file $container $framed $root
            write-file $notice_path $notice $root
            write-file $readme_path $readme $root
            print $"wrote ($fonts | length) fonts from typst-assets ($release.version)"
        }
    } finally {
        rm --recursive --force $work
    }
}
