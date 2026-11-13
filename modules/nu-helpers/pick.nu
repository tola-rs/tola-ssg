def detail-text [row: any]: nothing -> string {
    if ($row | describe --detailed | get type) == 'string' {
        $row
    } else {
        $row | to nuon --indent 2 --serialize
    }
}

def marked-cell [row: record, column: string, original: string, marked: bool]: nothing -> record {
    let value = ($row | get $original)
    let text = if $value == null { '' } else { detail-text $value }
    let mark = if $marked { '✓ ' } else { '  ' }
    $row | upsert $column $"($mark)($text)"
}

def toggle-mark [state: record, index_column: string, column: string, original: string]: nothing -> any {
    let row = $state.values.rows.row
    if $state.focused != 'rows' or $row == null { return }
    let selection = $state.values.marks.row
    let index = ($row | get $index_column)
    let marked = if $index in $selection.marked {
        $selection.marked | where {|item| $item != $index }
    } else {
        $selection.marked | append $index | sort
    }
    let candidates = $selection.candidates | update $index {|item|
        marked-cell $item $column $original ($index in $marked)
    }
    [{candidates: $candidates, marked: $marked, status: $"Marked: ($marked | length)"}]
}

# Pick original values from a searchable table. Cancel or pass an empty list to return [].
export def main [
    --title: string = 'Pick'
    --columns: list<string> = [] # Columns shown in the table.
    --search-columns: list<string> = [] # Empty searches all original fields.
    --fuzzy # Match a subsequence instead of a substring.
    --multi # Toggle marks with Space. Marks survive filtering.
    --preview: closure # Produce preview text from the original row.
]: list<any> -> list<any> {
    let rows = $in
    if ($rows | is-empty) { return [] }
    let records = $rows | each {|row|
        if ($row | describe --detailed | get type) == 'record' { $row } else { {item: $row} }
    }
    let fields = $records | each {|row| $row | columns } | flatten | uniq
    let all_columns = if ($fields | is-empty) { ['item'] } else { $fields }
    let table_columns = if ($columns | is-empty) { $all_columns | first 4 } else { $columns }
    let search_fields = if ($search_columns | is-empty) { $all_columns } else { $search_columns }
    let used = $all_columns | append $table_columns | append $search_fields
    mut index_column = '__index'
    while $index_column in $used { $index_column += '_' }
    mut original_column = '__first'
    while $original_column in $used { $original_column += '_' }
    let index_column = $index_column
    let original_column = $original_column
    let first_column = ($table_columns | first)
    let filter_columns = $search_fields | each {|field|
        if $multi and $field == $first_column { $original_column } else { $field }
    }
    let indexed = $records | enumerate | each {|row|
        let item = $row.item | insert $index_column $row.index
        if $multi {
            let item = ($item | insert $original_column ($item | get -o $first_column))
            marked-cell $item $first_column $original_column false
        } else {
            $item
        }
    }

    let preview_error = if $preview != null { mktemp -t 'tui-pick-preview.XXXXXXXX' }
    try {
        let detail = tui preview --id details --title Preview --from rows {|row|
            let original = ($rows | get ($row | get $index_column))
            if $preview == null { return (detail-text $original) }
            try {
                let text: string = (do $preview $original)
                $text
            } catch {|err|
                $err.msg | save --force --raw $preview_error
                $err.msg
            }
        }
        let search = tui search --bind / --placeholder 'Filter rows' --columns $filter_columns --fuzzy=$fuzzy
        let table = if $multi {
            tui table --id rows --focus --columns $table_columns --from marks {|row| $row.candidates }
        } else {
            tui table --id rows --focus --columns $table_columns
        }
        # Scope search to the candidates so the marked rows remain accessible.
        let candidates = tui box '' --border frameless [$search $table]
        let preview_pane = tui box '' --border frameless [$detail]
        let panes = [$candidates $preview_pane]
        let app = if $multi {
            [{candidates: $indexed, marked: [], status: 'Marked: 0'}]
            | tui label --titlebar $title
            | tui split --vertical --sizes ['1fr' 3] [
                (tui split $panes)
                (tui select --id marks --display status --title ' ' --border frameless)
            ]
            | tui label --status '/: filter · Tab: focus · Space: mark · Enter: pick · q: quit'
            | tui bind space {|state| toggle-mark $state $index_column $first_column $original_column }
        } else {
            $indexed | tui label --titlebar $title | tui split $panes
            | tui label --status '/: filter · Tab: focus · Enter: pick · q: quit'
        }
        let outcome = ($app | tui run)
        if $preview_error != null {
            let message = (open --raw $preview_error)
            if ($message | is-not-empty) { error make {msg: $"Preview failed: ($message)"} }
        }
        if $outcome.action != 'submit' { return [] }
        let marked = ($outcome.values.marks?.row?.marked? | default [])
        let picked = if ($marked | is-not-empty) {
            $marked
        } else if $outcome.values.rows.row == null {
            []
        } else {
            [($outcome.values.rows.row | get $index_column)]
        }
        $picked | each --keep-empty {|index| $rows | get $index }
    } finally {
        if $preview_error != null { rm --force $preview_error }
    }
}
