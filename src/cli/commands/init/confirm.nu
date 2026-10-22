use tola ui

def main [input_path: string, result_path: string] {
    let plan = (open --raw $input_path | from json)
    let body = $"Create site in ($plan.target)?(char nl)(char nl)($plan.files)"
    let outcome = (ui confirm --title 'Create site' --body $body --accept 'Create site' --cancel 'Cancel')
    let result = if $outcome.action == 'submit' and $outcome.selected == true {
        {action: submit, selected: true}
    } else {
        {action: quit}
    }
    $result | to json --raw | save --force $result_path
}
