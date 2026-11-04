# Highlight source text for a preview. `language` is a fence tag or file extension.
export def highlight [
    language: string
    --theme: string # dark, light, or plain; defaults to $env.TOLA_THEME.
]: string -> string {
    let theme = if $theme == null { $env.TOLA_THEME? | default 'dark' } else { $theme }
    $in | __tola_highlight $language --theme $theme
}

def value-type [value: any]: nothing -> string {
    $value | describe --detailed | get type
}

def format-value [value: any]: nothing -> string {
    let kind = (value-type $value)
    if $kind == 'list' {
        let scalar = $value | all {|item| (value-type $item) not-in ['record' 'list'] }
        if $scalar {
            $value | each {|item| $item | into string } | str join ', '
        } else {
            $value | to json --indent 2
        }
    } else if $kind == 'record' {
        if 'path' in ($value | columns) {
            let path = $value.path | into string
            let line = $value.line?
            if $line == null { $path } else { $'($path):($line)' }
        } else {
            $value | to json --indent 2
        }
    } else if $kind == 'nothing' {
        ''
    } else {
        $value | into string
    }
}

def truncate [text: string]: nothing -> string {
    if ($text | str length) > 60 {
        ($text | str substring 0..57) + '…'
    } else {
        $text
    }
}

def cell-value [value: any]: nothing -> string {
    let kind = (value-type $value)
    if $kind == 'list' {
        let count = ($value | length)
        let scalar = $value | all {|item| (value-type $item) not-in ['record' 'list'] }
        if $scalar {
            truncate ($value | each {|item| $item | into string } | str join ', ')
        } else {
            $'($count) items'
        }
    } else if $kind == 'record' {
        if 'path' in ($value | columns) {
            let path = $value.path | into string
            let line = $value.line?
            if $line == null { truncate $path } else { truncate $'($path):($line)' }
        } else {
            $'($value | columns | length) fields'
        }
    } else if $kind == 'nothing' {
        ''
    } else {
        truncate ($value | into string)
    }
}

def display-row [row: record]: nothing -> record {
    $row | transpose key value | reduce --fold {} {|field, shown|
        $shown | insert $field.key (cell-value $field.value)
    }
}

def default-columns [fields: list<string>]: nothing -> list<string> {
    let preferred = ['address' 'title' 'name' 'kind' 'taxon' 'tags' 'url' 'output' 'source']
    let present = $preferred | where {|name| $name in $fields }
    if ($present | is-empty) { $fields | first 4 } else { $present | first 4 }
}

def source-text [row: record]: nothing -> any {
    let path = $row.source?.path?
    if (value-type $path) != 'string' { return null }
    let kind = try { $path | path type } catch { '' }
    if $kind != 'file' { return null }
    let extension = try { $path | path parse | get extension | default '' } catch { '' }
    let text = try {
        open --raw $path | lines | first 200 | str join (char nl)
    } catch {
        null
    }
    if $text == null { return null }
    try { $text | highlight $extension } catch { null }
}

def detail-text [row: any]: nothing -> string {
    if (value-type $row) != 'record' { return ($row | into string) }
    let source = (source-text $row)
    if $source != null { return $source }
    let title = if ('address' in ($row | columns)) and ('title' in ($row | columns)) {
        $'($row.address) — ($row.title)'
    } else {
        ''
    }
    let skip = if $title == '' { [] } else { ['address' 'title'] }
    let body = $row
        | transpose key value
        | where {|field| $field.key not-in $skip }
        | each {|field| $'($field.key): (format-value $field.value)' }
        | str join (char nl)
    if $title == '' { $body } else { $title + (char nl) + (char nl) + $body }
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

# Choose one original value from a searchable table.
export def choose [
    --title: string = 'Choose'
    --columns: list<string> = [] # Columns shown in the table.
    --search-columns: list<string> = [] # Columns searched; empty searches all fields.
    --fuzzy # Match a subsequence instead of a substring.
    --multi # Choose several rows with Space.
    --preview: closure # Produce preview text from the original row.
]: list<any> -> record {
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
    let filter_columns = if ($search_columns | is-empty) { $all_columns } else { $search_columns }
    let indexed = $rows | enumerate | each {|row|
        let shown = if ($row.item | describe --detailed | get type) == 'record' {
            display-row $row.item
        } else {
            {item: (cell-value $row.item)}
        }
        $shown | insert $index_name $row.index
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
    let app = $indexed
        | tui label --titlebar $title
        | tui split --sizes ['55%' '1fr'] [
            (tui box Rows [
                (tui search --id search --bind / --placeholder 'Filter rows' --columns $filter_columns --fuzzy=$fuzzy)
                (tui table --id rows --focus --columns $table_columns --multi=$multi)
            ])
            $detail
        ]
        | tui label --status (if $multi {
            '/: filter · Space: mark · Enter: select · q: quit'
        } else {
            '/: filter · Tab: focus · Enter: select · q: quit'
        })
    let app = if $multi {
        $app
    } else {
        $app | tui bind enter {|state|
            if $state.values.rows.row? != null { {action: submit, selected: true} }
        }
    }
    let outcome = $app | tui run
    if $outcome.action != 'submit' { return {action: quit} }
    let picked = if $multi {
        if (($outcome.selected | describe) | str starts-with 'table') {
            $outcome.selected
        } else {
            [$outcome.selected]
        }
    } else {
        [$outcome.values.rows.row?]
    }
    if ($picked | is-empty) { return {action: quit} }
    let selected = $picked | each {|row| $rows | get ($row | get $index_name)}
    if $multi {
        {action: submit, selected: $selected}
    } else {
        {action: submit, selected: ($selected | first)}
    }
}
