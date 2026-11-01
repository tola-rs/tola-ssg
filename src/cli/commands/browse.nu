use tola ui

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
        $input.rows | ui choose --title $input.title
    } else {
        let outcome = (value-browser $input | tui run)
        if $outcome.action == submit {
            {action: submit, selected: $input.value}
        } else {
            {action: quit}
        }
    }
    $result | to json --raw | save --raw --force $result_json
}
