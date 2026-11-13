def main [input_path: string, result_path: string] {
    let plan = (open --raw $input_path | from json)
    let body = $"Create site in ($plan.target)?(char nl)(char nl)($plan.files)"
    let choices = [{label: 'Cancel', value: false} {label: 'Create site', value: true}]
    let outcome = $choices
        | tui label --titlebar 'Create site'
        | tui box Plan [
            (tui preview --title Details {|row| $body})
            (tui select --id rows --focus --display label)
        ]
        | tui label --status 'Enter: choose · q: cancel'
        | tui run
    let result = if $outcome.action == 'submit' and $outcome.selected.value? == true {
        {action: submit, selected: true}
    } else {
        {action: quit}
    }
    $result | to json --raw | save --force $result_path
}
