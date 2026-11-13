use tola [nu-helpers syntax-highlight]

def value-browser [input: record] {
    let view = if $input.value == null {
        tui label 'null'
    } else {
        $input.value | tui tree --id value --title 'Query result'
    }
    $view
    | tui label --titlebar $input.title
    | tui label --status '←/→: expand · Enter: return result · q: quit'
    | tui bind enter {|| {action: submit, selected: $input.value}}
}

def main [input_json: path, result_json: path] {
    let input = open --raw $input_json | from json
    let result = if 'rows' in $input {
        let picked = ($input.rows | nu-helpers pick --title $input.title --preview {|row|
            $row | to json --indent 2 | syntax-highlight json --theme terminal
        })
        if ($picked | is-empty) {
            {action: quit}
        } else {
            {action: submit, selected: ($picked | first)}
        }
    } else {
        let outcome = (value-browser $input | tui run)
        if $outcome.action == submit {
            {action: submit, selected: $outcome.selected}
        } else {
            {action: quit}
        }
    }
    $result | to json --raw | save --raw --force $result_json
}
