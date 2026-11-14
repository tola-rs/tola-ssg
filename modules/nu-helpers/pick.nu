def detail-text [value: any]: nothing -> string {
    if ($value | describe) == 'string' {
        $value
    } else {
        $value | to nuon --indent 2 --serialize
    }
}

def unused-column [name: string, columns: list<string>]: nothing -> string {
    mut column = $name
    while $column in $columns { $column += '_' }
    $column
}

def marked-text [value: any, marked: bool]: nothing -> string {
    let text = if $value == null { '' } else { detail-text $value }
    let mark = if $marked { '✓ ' } else { '  ' }
    $"($mark)($text)"
}

def toggle-mark [state: record, index_column: string, first_column: string, original_column: string]: nothing -> any {
    let row = $state.values.rows.row
    if $state.focused != 'rows' or $row == null { return }
    let selection = $state.values.marks.row
    let index = ($row | get $index_column)
    let was_marked = $index in $selection.marked
    let marked = if $was_marked {
        $selection.marked | where {|item| $item != $index }
    } else {
        $selection.marked | append $index
    }
    let candidates = $selection.candidates | update $index {|candidate|
        let value = ($candidate | get $original_column)
        $candidate | update $first_column (marked-text $value (not $was_marked))
    }
    [{candidates: $candidates, marked: $marked}]
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
        match $row {
            {} => $row
            _ => { {item: $row} }
        }
    }
    let all_columns = ($records | columns | default --empty ['item'])
    let table_columns = if ($columns | is-empty) { $all_columns | first 4 } else { $columns }
    let search_fields = if ($search_columns | is-empty) { $all_columns } else { $search_columns }
    let used = $all_columns | append $table_columns | append $search_fields
    let index_column = (unused-column '__index' $used)
    let original_column = (unused-column '__first' $used)
    let first_column = ($table_columns | first)
    # Search the original value even when the first cell displays a check mark.
    let filter_columns = $search_fields | each {|field|
        if $multi and $field == $first_column { $original_column } else { $field }
    }
    let indexed = $records | enumerate | each {|row|
        let item = $row.item | insert $index_column $row.index
        if $multi {
            let value = ($item | get -o $first_column)
            $item | insert $original_column $value | upsert $first_column (marked-text $value false)
        } else {
            $item
        }
    }

    let preview_error = if $preview != null { mktemp -t 'tui-pick-preview.XXXXXXXX' }
    let outcome = (try {
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
            [{candidates: $indexed, marked: []}]
            | tui label --titlebar $title
            | tui split --vertical --sizes ['1fr' 3] [
                (tui split $panes)
                (tui select --id marks --title ' ' --border frameless --display {|row|
                    $"Marked: ($row.marked | length)"
                })
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
        $outcome
    } finally {
        if $preview_error != null { rm --force $preview_error }
    })

    if $outcome.action != 'submit' { return [] }
    let marked = ($outcome.values.marks?.row?.marked? | default [])
    let picked = if ($marked | is-not-empty) {
        $marked | sort
    } else {
        [($outcome.values.rows.row | get -o $index_column)] | compact
    }
    $picked | each --keep-empty {|index| $rows | get $index }
}
