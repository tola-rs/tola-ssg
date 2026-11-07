#!/usr/bin/env nu
# Read a context document by section, or check its heading structure.

def read-doc [file] {
    let raw = (open --raw $file)
    let lines = ($raw | lines)
    let heads = (
        $raw
        | from md --verbose
        | where {|n| $n.type =~ '^h[1-6]$'}
        | each {|n|
            let line = $n.position.start.line
            let level = $n.attrs.level
            let title = (
                $lines
                | get ($line - 1)
                | str trim
                | str replace -r '\s+#+$' ''
                | str replace -r '^#+\s*' ''
                | str trim
            )
            {level: $level, line: $line, title: $title}
        }
    )
    {lines: $lines, heads: $heads}
}

def with-paths [heads] {
    $heads | reduce --fold {stack: [], out: []} {|h, acc|
        if $h.level == 1 {
            {
                stack: [],
                out: ($acc.out | append {level: $h.level, line: $h.line, title: $h.title, path: []})
            }
        } else {
            let stack = ($acc.stack | where {|s| $s.level < $h.level})
            let path = (($stack | each {|s| $s.title}) | append $h.title)
            {
                stack: ($stack | append $h),
                out: ($acc.out | append {level: $h.level, line: $h.line, title: $h.title, path: $path})
            }
        }
    } | get out
}

def with-ends [records, lines] {
    let limit = ($lines | length) + 1
    $records | enumerate | each {|it|
        let next = ($records | get -o ($it.index + 1))
        let end = (if ($next | is-empty) { $limit } else { $next.line })
        $it.item | merge {end: $end}
    }
}

def doc-records [doc] {
    with-ends (with-paths $doc.heads) $doc.lines
}

def body-lines [record, lines] {
    let start = $record.line
    let end = ($record.end - 2)
    if $end < $start { [] } else { $lines | slice $start..$end }
}

def trim-edges [lines] {
    $lines | str join "\n" | str replace -r '^\n+' '' | str replace -r '\n+$' ''
}

def indent [n] {
    if $n <= 0 { "" } else { (1..$n | each { "  " } | str join "") }
}

def usage [] {
    print "usage: just ctx FILE [TOPIC [SECTION]]"
    print "       just ctx --check FILE"
    print "TOPIC and SECTION are heading titles, space-separated; PARENT / CHILD also resolves."
}

def not-found [file, path, paths] {
    print -e $"path not found: ($path | str join ' ')"
    let last = ($path | last)
    let hits = (
        $paths
        | where {|p| ($p | last) == $last}
        | each {|p| $p | str join " / "}
        | uniq
    )
    if ($hits | is-empty) {
        print -e $"run: just ctx ($file)"
    } else {
        for h in $hits {
            print -e $"  did you mean: ($h)"
        }
    }
    exit 1
}

def list-all [file] {
    let doc = (read-doc $file)
    let records = (doc-records $doc)
    let h1s = ($records | where {|r| $r.level == 1})
    if ($h1s | is-not-empty) {
        let body = (trim-edges (body-lines ($h1s | first) $doc.lines))
        if ($body | is-not-empty) {
            print $body
            print ""
        }
    }

    for r in ($records | where {|r| $r.level >= 2}) {
        let pad = (indent (($r.level - 2)))
        print $"($pad)($r.title)"
    }
}

def show-section [file, path] {
    let doc = (read-doc $file)
    let records = (doc-records $doc)
    let key = ($path | str join " ")
    let exact = ($records | where {|r| ($r.path | str join " / ") == $key})
    let matches = (
        if ($exact | is-empty) {
            $records | where {|r| ($r.path | str join " ") == $key}
        } else {
            $exact
        }
    )

    if ($matches | is-empty) {
        not-found $file $path ($records | each {|r| $r.path} | where {|p| ($p | length) > 0})
    }

    if ($matches | length) > 1 {
        print -e $"ambiguous path: ($key)"
        for m in $matches {
            print -e $"  ($m.path | str join ' / ')"
        }
        exit 1
    }

    let target = ($matches | first)
    let body = (trim-edges (body-lines $target $doc.lines))
    if ($body | is-not-empty) {
        print $body
    }

    let depth = ($target.path | length)
    let children = (
        $records
        | where {|r| ($r.path | length) == ($depth + 1) }
        | where {|r| ($r.path | first $depth) == $target.path }
    )

    if ($children | is-not-empty) {
        if ($body | is-not-empty) {
            print ""
        }
        for child in $children {
            print $"- ($child.title)"
        }
    }
}

def check-file [file] {
    let doc = (read-doc $file)
    let heads = $doc.heads
    let records = (doc-records $doc)
    mut errors = []

    let h1s = ($records | where {|r| $r.level == 1})
    if ($h1s | length) != 1 {
        $errors = ($errors | append $"expected exactly one level-1 title, found ($h1s | length)")
    }

    if ($heads | is-not-empty) {
        let first_level = ($heads | first | get level)
        if $first_level != 1 {
            $errors = ($errors | append "first heading is not level 1")
        }
    }

    if ($h1s | is-not-empty) {
        let body = (body-lines ($h1s | first) $doc.lines)
        let nonblank = ($body | any {|l| ($l | str trim | is-not-empty) })
        if not $nonblank {
            $errors = ($errors | append "missing preamble under the level-1 title")
        }
    }

    for r in ($records | where {|r| ($r.path | length) != ($r.level - 1)}) {
        $errors = ($errors | append $"level jump: ($r.title) (level ($r.level), depth ($r.path | length))")
    }

    for r in ($records | where {|r| ($r.title | str trim | is-empty)}) {
        $errors = ($errors | append "empty title")
    }

    let dupes = ($records | each {|r| $r.path | str join " / "} | sort | uniq -d)
    for d in $dupes {
        $errors = ($errors | append $"duplicate title: ($d)")
    }

    if ($errors | is-empty) {
        print $"ok: ($file)"
    } else {
        print -e ($errors | str join "\n")
        exit 1
    }
}

# Print a document's headings, one section named by TOPIC and SECTION, or check its structure.
def main [
    --check
    file?: string
    ...path: string
] {
    if ($file | is-empty) {
        usage
        exit 1
    }

    if $check {
        if ($path | is-not-empty) {
            print -e "--check takes only a file"
            exit 1
        } else {
            check-file $file
        }
    } else if ($path | is-empty) {
        list-all $file
    } else {
        show-section $file $path
    }
}
