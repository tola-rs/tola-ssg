#!/usr/bin/env nu
# Regenerate a pronunciation table from the source file it is derived from.
#
# Usage: nu tools/assets/pronunciations.nu <source> <path or url>
#
# Sources
#   unihan  Han character pronunciations, from the Unihan database. Pass the
#           Unihan_Readings.txt extracted from the Unihan.zip the Unicode site links to.
#   cedict  Chinese word pronunciations, from CC-CEDICT (CC BY-SA 4.0). Pass
#           https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz
#   jmdict  Japanese word pronunciations, from JMdict (CC BY-SA 4.0, EDRDG terms). Pass
#           https://www.edrdg.org/pub/Nihongo/JMdict_e.gz
#
# A path may be a local file or an http(s) URL, and either may be gzipped. The table is
# committed behind one zstd frame a build decodes without fetching anything. Names are
# ordered by their UTF-8 bytes, which is the code-point order a lookup compares.

use ../lib/assets.nu *

const UNIHAN_URL = 'https://www.unicode.org/Public/UCD/latest/ucd/Unihan.zip'
const CEDICT_URL = 'https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz'
const JMDICT_URL = 'https://www.edrdg.org/pub/Nihongo/JMdict_e.gz'
const DATA = 'crates/tola-pronunciations/data'

# The tone-marked letters Hanyu Pinyin spells its syllables with, folded to their base letter:
# Nu has no Unicode normalization, so the precomposed spellings are named here.
const TONES = [
    ['ā' 'a'] ['á' 'a'] ['ǎ' 'a'] ['à' 'a']
    ['ē' 'e'] ['é' 'e'] ['ě' 'e'] ['è' 'e'] ['ê' 'e']
    ['ī' 'i'] ['í' 'i'] ['ǐ' 'i'] ['ì' 'i']
    ['ō' 'o'] ['ó' 'o'] ['ǒ' 'o'] ['ò' 'o']
    ['ū' 'u'] ['ú' 'u'] ['ǔ' 'u'] ['ù' 'u']
    ['ǖ' 'u'] ['ǘ' 'u'] ['ǚ' 'u'] ['ǜ' 'u'] ['ü' 'u']
    ['ń' 'n'] ['ň' 'n'] ['ǹ' 'n']
    ['ḿ' 'm']
]

# The source text: a local file or a URL, either of which may be gzipped.
def read-source [source: string, work: string] {
    let gzipped = ($source | str ends-with '.gz')
    if ($source =~ '^https?://') {
        let downloaded = ($work | path join 'source')
        download $source $downloaded
        if $gzipped { ^gzip -d -c $downloaded } else { open --raw $downloaded }
    } else if not ($source | path exists) {
        error make { msg: $"($source) names no file" }
    } else if $gzipped {
        ^gzip -d -c $source
    } else {
        open --raw $source
    }
}

# The version and date lines the Unihan file carries.
def unihan-provenance [source: string] {
    mut facts = []
    for line in ($source | lines) {
        if not ($line | str starts-with '#') { break }
        let field = ($line | str substring 1.. | str trim)
        if ($field | str starts-with 'Date:') or ($field | str starts-with 'Unicode Version') {
            $facts = ($facts | append $field)
        }
    }
    if ($facts | is-empty) { error make { msg: 'the source file does not say which version it is' } }
    $facts
}

# The release facts the CC-CEDICT file carries in its `#!` header.
def cedict-provenance [source: string] {
    let wanted = ['entries' 'publisher' 'license' 'date']
    mut facts = []
    for line in ($source | lines) {
        if not ($line | str starts-with '#!') { continue }
        let separator = ($line | str index-of '=')
        if $separator < 0 { continue }
        let name = ($line | str substring 2..<$separator | str trim)
        if not ($name in $wanted) { continue }
        $facts = ($facts | append $"($name): ($line | str substring ($separator + 1).. | str trim)")
    }
    if ($facts | is-empty) { error make { msg: 'the source file does not say which version it is' } }
    $facts
}

# The release fact the JMdict file carries: the day it was created.
def jmdict-provenance [source: string] {
    let created = ($source | parse -r '(?i)<!--\s*JMdict created:\s*(?<date>[\d-]+)\s*-->' | get -o 0.date)
    if $created == null { error make { msg: 'the source file does not say which version it is' } }
    [$"Created: ($created)"]
}

# The customary Mandarin pronunciation of one character, without its tone mark.
def mandarin [value: string] {
    let first = (
        $value | str trim | split row --regex '\s+' | first | default ''
        | str lowercase | str replace --all --regex '\p{Mn}' ''
    )
    let toneless = (
        $first | split chars | each {|character|
            let tone = ($TONES | where {|entry| $entry.0 == $character } | get -o 0)
            if $tone == null { $character } else { $tone.1 }
        } | str join
    )
    if not ($toneless =~ '^[a-z]+$') {
        error make { msg: $"the source pronunciation `($value)` is not a toneless syllable" }
    }
    $toneless
}

# Every kMandarin pronunciation in the Unihan readings file.
def unihan [source: string] {
    let entries = (
        $source
        | lines
        | where {|line| not ($line | str starts-with '#')}
        | each {|line|
            let columns = ($line | split row "\t")
            if ($columns | get -o 1) != 'kMandarin' {
                null
            } else {
                let code = ($columns | get -o 0)
                let value = ($columns | get -o 2)
                if $code == null or $value == null {
                    error make { msg: $"the source line `($line)` is not a Unihan entry" }
                }
                {
                    name: (char -i ($code | str substring 2.. | into int --radix 16)),
                    pronunciation: (mandarin $value)
                }
            }
        }
        | compact
        | sort-by name
    )
    if ($entries | is-empty) { error make { msg: 'the source file names no pronunciation' } }
    {
        stem: 'unihan',
        title: 'Han character pronunciations',
        command: 'just tools::pronunciations unihan <Unihan_Readings.txt>',
        notice: [
            $"Source: ($UNIHAN_URL)"
            'Licence: Unicode License v3, https://www.unicode.org/license.txt'
            'Changes: kMandarin only; the first pronunciation of each value;'
            '         tone marks and the diaeresis in ü removed; lowercased; sorted by code point.'
        ],
        provenance: (unihan-provenance $source),
        entries: $entries
    }
}

# One word's pronunciation: syllables carry no tone and no punctuation.
def word-pronunciation [value: string] {
    let syllables = (
        $value
        | str trim
        | split row --regex '\s+'
        | each {|syllable|
            $syllable
            | str replace --all --regex '\d' ''
            | str replace --all 'u:' 'u'
            | str replace --all 'v' 'u'
            | str lowercase
        }
    )
    if ($syllables | is-empty) or ($syllables | any {|syllable| not ($syllable =~ '^[a-z]+$')}) {
        return null
    }
    $syllables | str join ' '
}

# Every word pronunciation in the CC-CEDICT file, in both of its spellings.
def cedict [source: string] {
    let entries = (
        $source
        | lines
        | where {|line| not ($line | str starts-with '#') and ($line | str trim) != ''}
        | each {|line|
            let parsed = ($line | parse -r '^(?<traditional>\S+) (?<simplified>\S+) \[(?<pinyin>[^\]]+)\] /')
            if ($parsed | is-empty) {
                error make { msg: $"the source line `($line)` is not a CC-CEDICT entry" }
            }
            let entry = ($parsed | first)
            let pronunciation = (word-pronunciation $entry.pinyin)
            if $pronunciation == null {
                []
            } else {
                [$entry.traditional $entry.simplified]
                | where {|word| (($word | split chars | length) >= 2) and ($word =~ '\p{Han}')}
                | each {|word| { name: $word, pronunciation: $pronunciation } }
            }
        }
        | flatten
        | uniq-by name
        | sort-by name
    )
    if ($entries | is-empty) { error make { msg: 'the source file names no word' } }
    {
        stem: 'cedict',
        title: 'Chinese word pronunciations',
        command: 'just tools::pronunciations cedict <cedict_1_0_ts_utf-8_mdbg.txt.gz>',
        notice: [
            $"Source: CC-CEDICT, ($CEDICT_URL)"
            'Licence: Creative Commons Attribution-ShareAlike 4.0 International,'
            '         https://creativecommons.org/licenses/by-sa/4.0/'
            'Copyright (C) 1997, 1998 Paul Andrew Denisowski and the CC-CEDICT contributors.'
            'Changes: both spellings; keys of at least two Unicode scalar values containing Han;'
            '         tone digits removed; u: and v (spellings of ü) folded to u;'
            '         unsupported pronunciations omitted; the first entry for a word wins; sorted by name.'
        ],
        provenance: (cedict-provenance $source),
        entries: $entries
    }
}

# Every word pronunciation in the JMdict file.
def jmdict [source: string] {
    let entries = (
        $source
        | parse -r '(?s)<entry>(?<entry>.*?)</entry>'
        | get -o entry
        | default []
        | each {|block|
            let kanji = ($block | parse -r '<keb>(?<word>[^<]+)</keb>' | get -o word | default [])
            $block
            | parse -r '(?s)<r_ele>(?<reading>.*?)</r_ele>'
            | get -o reading
            | default []
            | each {|reading|
                if ($reading =~ '<re_nokanji[^>]*>') {
                    []
                } else {
                    let reb = ($reading | parse -r '<reb>(?<kana>[^<]+)</reb>' | get -o 0.kana)
                    if $reb == null or not ($reb =~ '^[\u{3041}-\u{309e}\u{30a1}-\u{30fe}\u{30fc}]+$') {
                        []
                    } else {
                        let restrictions = ($reading | parse -r '<re_restr>(?<word>[^<]+)</re_restr>' | get -o word | default [])
                        $kanji
                        | where {|word| ($word =~ '\p{Han}') and (($restrictions | is-empty) or ($word in $restrictions))}
                        | each {|word| { name: $word, pronunciation: $reb } }
                    }
                }
            }
            | flatten
        }
        | flatten
        | uniq-by name
        | sort-by name
    )
    if ($entries | is-empty) { error make { msg: 'the source file names no word' } }
    {
        stem: 'jmdict',
        title: 'Japanese word pronunciations',
        command: 'just tools::pronunciations jmdict <JMdict_e.gz>',
        notice: [
            $"Source: JMdict, ($JMDICT_URL)"
            'Licence: Creative Commons Attribution-ShareAlike 4.0 International,'
            '         https://creativecommons.org/licenses/by-sa/4.0/, with the terms of'
            '         https://www.edrdg.org/edrdg/licence.html'
            'Copyright © Electronic Dictionary Research and Development Group (EDRDG).'
            'Changes: spellings containing Han use their first supported kana reading,'
            '         respecting re_restr and re_nokanji; spellings are kept as in the source;'
            '         the first entry for a spelling wins; sorted by spelling.'
        ],
        provenance: (jmdict-provenance $source),
        entries: $entries
    }
}

# The table's header: its title, the source's own version, its notice, and the command to rerun.
def header [table: record] {
    let lines = (
        [$"# ($table.title)"]
        ++ ($table.provenance | each {|line| $"# ($line)"})
        ++ ($table.notice | each {|line| $"# ($line)"})
        ++ [$"# Generated by `($table.command)`; regenerate it instead of editing it." '']
    )
    $lines | str join (char newline)
}

def write-table [root: string, table: record, work: string] {
    let path = ($root | path join $DATA $"($table.stem).zst")
    let body = ($table.entries | each {|entry| $"($entry.name)\t($entry.pronunciation)"})
    let text = ((header $table) + ($body | str join (char newline)) + (char newline))
    let raw = ($work | path join 'table.txt')
    $text | save --force $raw
    compress-zstd $raw $path
    print $"($path): ($body | length) pronunciations"
}

def main [source?: string, location?: string] {
    if $source == null or $location == null {
        print 'usage: nu tools/assets/pronunciations.nu <unihan|cedict|jmdict> <path or url>'
        if ($source == null) and ($location == null) { return }
        exit 1
    }
    let root = ($env.FILE_PWD | path dirname | path dirname)
    let work = (mktemp --directory)
    try {
        let text = (read-source $location $work)
        let table = (
            if $source == 'unihan' {
                unihan $text
            } else if $source == 'cedict' {
                cedict $text
            } else if $source == 'jmdict' {
                jmdict $text
            } else {
                error make { msg: $"`($source)` is not a source this command generates" }
            }
        )
        write-table $root $table $work
    } finally {
        rm --recursive --force $work
    }
}
