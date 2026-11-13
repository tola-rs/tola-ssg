def node-page [data: record, node: any] {
    if $node == [] {
        'Overview'
    } else if (($node | describe --detailed | get type) == 'string') {
        $node
    } else {
        $data.groups | where {|group| $group.pages == $node } | get -o 0.name
    }
}

def browse [data: record, enter: string] {
    # An empty child list displays its key as a leaf in the tree.
    let groups = ($data.groups | reduce -f {Overview: []} {|group, acc|
        $acc | insert $group.name $group.pages
    })
    let tree = if $data.last_page == null {
        $groups
    } else {
        {Recent: $data.last_page} | merge $groups
    }
    let app = ($tree
        | tui label --titlebar $data.title
        | tui split --sizes [$data.contents_columns '1fr'] [
            (tui tree --id tree --title Contents --focus)
            (tui preview --title Page --from tree {|node|
                let page = (node-page $data $node)
                if $page != null { $data.bodies | get $page }
            })
        ]
        | tui label --status $'/: search · ↑/↓: move · →/←: expand · Enter: ($enter) · q: quit'
        | tui bind / {|| {action: submit, selected: {search: true}}}
        | tui bind enter {|state|
            let page = (node-page $data $state.values.tree.value)
            if $page != null {
                {action: submit, selected: {page: $page, target: ($data.targets | get $page)}}
            }
        })
    let outcome = ($app | tui run)
    if $outcome.action == 'submit' { $outcome.selected }
}

def excerpt [text: string, query: string] {
    let index = ($text | str lowercase | str index-of --grapheme-clusters $query)
    let start = ([($index - 24) 0] | math max)
    let end = ($start + 96)
    let left = if $start > 0 { '…' } else { '' }
    let right = if ($text | str length --grapheme-clusters) > $end { '…' } else { '' }
    $'($left)($text | str substring --grapheme-clusters $start..<$end)($right)'
}

def matches [sections: list, raw: string] {
    let query = ($raw | str trim | str lowercase | str replace --all --regex '\s+' ' ')
    if ($query | is-empty) { return [] }
    let pattern = ($query | split chars | each {|char|
        if $char in ['\' '.' '^' '$' '*' '+' '?' '(' ')' '[' ']' '{' '}' '|'] {
            '\' + $char
        } else {
            $char
        }
    } | str join '.*?')
    $sections | each {|row|
        let title = if $row.section == '' { $row.page } else { $row.section }
        let name = ($title | str lowercase)
        let paragraph = ($row.paragraphs | where {|text| $text | str contains --ignore-case $query } | first 1 | get -o 0)
        let score = if $name == $query or ($name | str starts-with $'($query) - ') {
            0
        } else if ($name | str contains $query) {
            1
        } else if $paragraph != null {
            2
        } else if $name =~ $'^($pattern)' {
            3
        } else if $name =~ $pattern {
            4
        } else {
            return null
        }
        let text = $paragraph | default ($row.paragraphs | get -o 0 | default '')
        {
            page: $row.page
            section: $row.section
            excerpt: (excerpt $text $query)
            target: $row.target
            score: $score
            width: (if $score >= 3 {
                $name | parse --regex ('(?<match>' + $pattern + ')') | get match.0 | str length
            } else {
                $title | str length
            })
            query: $raw
        }
    } | sort-by score width page section
}

def search [data: record, enter: string] {
    mut query = ''
    loop {
        let app = (results $data.sections $query
            | tui label --titlebar 'Search help'
            | tui textbox --id query --title Search --value $query --focus --placeholder 'Search titles and text'
            | tui table --id results --title Results --columns [page section excerpt]
            | tui label --status $'Tab: results · ↑/↓: move · Enter: ($enter) · Ctrl+G: back'
            | tui bind ctrl+g {|| {action: quit}})
        let outcome = ($app | tui run --refresh 100ms {|state|
            let text = $state.values.query
            if ($state.values.results.row.query? | default null) != $text {
                results $data.sections $text
            }
        })
        if $outcome.action != 'submit' { return null }
        $query = $outcome.values.query
        let row = $outcome.values.results.row?
        let picked = if ($row.query? | default null) == $query {
            $row
        } else {
            matches $data.sections $query | get -o 0
        }
        if $picked.target? != null { return ($picked | select page target) }
    }
}

def results [sections: list, query: string] {
    let rows = (matches $sections $query)
    if ($rows | is-not-empty) { return $rows }
    let message = if ($query | str trim | is-empty) { '' } else { 'No matches' }
    [{page: '', section: '', excerpt: $message, query: $query}]
}

def main [input: path, output: path] {
    let data = (open --raw $input | from json)
    let enter = if $data.paged { 'read in pager' } else { 'print page' }
    mut result = {action: quit}
    loop {
        let choice = (browse $data $enter)
        if $choice == null { break }
        let picked = if ($choice.search? | default false) {
            search $data $enter
        } else {
            $choice
        }
        if $picked != null {
            $result = {action: submit, selected: $picked}
            break
        }
    }
    $result | to json --raw | save --force $output
}
