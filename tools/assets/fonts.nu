#!/usr/bin/env nu
# Regenerate the fonts this repository carries from the pinned typst-assets release.
#
# The version and checksum come from Cargo.lock. The crate is downloaded, verified, and
# unpacked, then the container, NOTICE, and provenance are rewritten. Passing --check compares
# the committed index and decoded payload without rewriting the carried files.

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
    let entries = ($lock | from toml | get -o package | default [] | where name == 'typst-assets')
    if ($entries | length) > 1 { error make { msg: 'Cargo.lock names multiple typst-assets releases' } }
    let entry = ($entries | get -o 0)
    if $entry == null { error make { msg: 'Cargo.lock names no locked typst-assets release' } }
    let checksum = ($entry.checksum? | default '')
    if not ($checksum =~ '^[0-9a-f]{64}$') {
        error make { msg: $"Cargo.lock has no valid checksum for typst-assets ($entry.version)" }
    }
    { version: $entry.version, sha256: $checksum }
}

# The little-endian u32 spelling of a length or offset.
def le32 [value: int] {
    bytes build ($value mod 256) (($value // 256) mod 256) (($value // 65536) mod 256) (($value // 16777216) mod 256)
}

# An index naming each font's offset and length: the container before its frame.
def indexed [fonts: list] {
    mut index = (bytes build ($MAGIC | into binary) (le32 ($fonts | length)))
    mut offset = 0
    for font in $fonts {
        let name = ($font.name | into binary)
        let entry = (bytes build ($name | bytes length) $name (le32 $offset) (le32 $font.length))
        $index = ($index ++ $entry)
        $offset += $font.length
    }
    $index
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

# The committed payload's digest if the index matches.
def decoded-hash [path: string, index: binary, work: string] {
    if not ($path | path exists) { return null }
    let container = (open --raw $path | into binary)
    if not ($container | bytes starts-with $index) { return null }
    let frame = ($work | path join 'committed.zst')
    $container | bytes at ($index | bytes length).. | save --force $frame
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
                let bytes = (open --raw $path | into binary)
                { name: $name, bytes: $bytes, length: ($bytes | bytes length), sha256: ($bytes | hash sha256) }
            }
        )
        let payload = ($fonts | each {|font| $font.bytes } | bytes collect)
        let index = (indexed $fonts)

        let notice = (open --raw ($work | path join 'NOTICE'))
        let readme = (provenance $release.version $release.sha256 $fonts)
        let container = ($root | path join $CONTAINER)
        let notice_path = ($root | path join $NOTICE)
        let readme_path = ($root | path join $PROVENANCE)

        if $check {
            mut stale = []
            let decoded = (decoded-hash $container $index $work)
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
            let raw = ($work | path join 'payload.bin')
            $payload | save --force $raw
            let frame = ($work | path join 'payload.zst')
            compress-zstd $raw $frame
            let framed = ([$index (open --raw $frame | into binary)] | bytes collect)
            write-file $container $framed $root
            write-file $notice_path $notice $root
            write-file $readme_path $readme $root
            print $"wrote ($fonts | length) fonts from typst-assets ($release.version)"
        }
    } finally {
        rm --recursive --force $work
    }
}
