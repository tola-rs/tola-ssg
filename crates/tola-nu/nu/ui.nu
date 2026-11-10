export alias highlight = __tola_highlight

def default-columns [fields: list<string>]: nothing -> list<string> {
    let preferred = ['address' 'title' 'name' 'kind' 'taxon' 'tags' 'url' 'output' 'source']
    let present = $preferred | where {|name| $name in $fields }
    if ($present | is-empty) { $fields | first 4 } else { $present | first 4 }
}

def detail-text [row: any]: nothing -> string {
    if ($row | describe --detailed | get type) == 'string' {
        $row
    } else {
        $row | to nuon --indent 2 --serialize
    }
}

# Confirm a prepared action. The first row is the safe cancel choice.
export def confirm [
    --title: string = 'Confirm'
    --body: string = '' # Details shown above the choices.
    --accept: string = 'Confirm'
    --cancel: string = 'Cancel'
]: nothing -> record {
    let choices = [
        {label: $cancel, value: false}
        {label: $accept, value: true}
    ]
    let body = $body
    let outcome = $choices
        | tui label --titlebar $title
        | tui box Plan [
            (tui preview --title Details {|row| $body})
            (tui select --id rows --focus --display label)
        ]
        | tui label --status 'Enter: choose · q: cancel'
        | tui run
    if $outcome.action == 'submit' and $outcome.selected? != null {
        {action: submit, selected: $outcome.selected.value}
    } else {
        {action: quit}
    }
}

# Choose original values from a table. Search is available in single selection.
export def choose [
    --title: string = 'Choose'
    --columns: list<string> = [] # Columns shown in the table.
    --search-columns: list<string> = [] # Columns searched in single selection; empty searches all fields.
    --fuzzy # Match a subsequence instead of a substring in single selection.
    --multi # Choose several rows with Space, without search.
    --preview: closure # Produce preview text from the original row.
]: list<any> -> record {
    if $multi and ($fuzzy or not ($search_columns | is-empty)) {
        error make {msg: '--fuzzy and --search-columns require single selection'}
    }
    let rows = $in
    if ($rows | is-empty) { return {action: quit} }
    let fields = $rows | enumerate | each {|row|
        if ($row.item | describe --detailed | get type) == 'record' {
            $row.item | columns
        } else {
            ['item']
        }
    } | flatten | uniq
    mut private_name = '__tola_ui_index'
    while $private_name in ($fields | append $columns | append $search_columns) {
        $private_name += '_'
    }
    let index_name = $private_name
    let all_columns = if ($fields | is-empty) { ['item'] } else { $fields }
    let table_columns = if ($columns | is-empty) { default-columns $all_columns } else { $columns }
    let indexed = $rows | enumerate | each {|row|
        let item = if ($row.item | describe --detailed | get type) == 'record' {
            $row.item
        } else {
            {item: $row.item}
        }
        $item | insert $index_name $row.index
    }
    let detail = if $preview == null {
        tui preview --id details --title Details --from rows {|row|
            detail-text ($rows | get ($row | get $index_name))
        }
    } else {
        tui preview --id details --title Preview --from rows {|row|
            do $preview ($rows | get ($row | get $index_name))
        }
    }
    let table = (tui table --id rows --focus --columns $table_columns --multi=$multi)
    let widgets = if $multi {
        # Nu 0.116.1 keeps checks by filtered position when the query changes.
        [$table]
    } else {
        let filter_columns = if ($search_columns | is-empty) { $all_columns } else { $search_columns }
        let search = (tui search --bind / --placeholder 'Filter rows' --columns $filter_columns --fuzzy=$fuzzy)
        [$search $table]
    }
    let outcome = $indexed
        | tui label --titlebar $title
        | tui split --sizes ['55%' '1fr'] [
            (tui box Rows $widgets)
            $detail
        ]
        | tui label --status (if $multi {
            'Space: mark · Enter: select · q: quit'
        } else {
            '/: filter · Tab: focus · Enter: select · q: quit'
        })
        | tui run
    if $outcome.action != 'submit' { return {action: quit} }
    let state = $outcome.values.rows
    let checked = ($state.checked? | default [])
    let picked = if not ($checked | is-empty) {
        $checked
    } else {
        if $state.row == null { return {action: quit} }
        [$state.row]
    }
    let selected = $picked | each --keep-empty {|row| $rows | get ($row | get $index_name)}
    if $multi {
        {action: submit, selected: $selected}
    } else {
        {action: submit, selected: ($selected | first)}
    }
}
