use tola ui

def main [input: path, output: path] {
    let choices = (open --raw $input | from json)
    let picked = ($choices | ui pick --title 'Select editors' --display editor)
    let result = if $picked.action == 'submit' {
        {action: submit, selected: ($picked.selected | each {|item| $item.index })}
    } else {
        {action: quit}
    }
    $result | to json --raw | save --force $output
}
