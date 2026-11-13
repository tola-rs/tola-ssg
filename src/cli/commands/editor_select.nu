use tola nu-helpers

def main [input: path, output: path] {
    let choices = (open --raw $input | from json)
    let picked = ($choices | nu-helpers pick --multi --columns [editor] --search-columns [editor settings] --title 'Select editors' --preview {|row|
        if $row.settings == null {
            $"($row.editor)\n\nCopy the settings printed by Tola into your editor configuration."
        } else {
            $"($row.editor)\n\nTola merges the workspace settings into ($row.settings)."
        }
    })
    let result = if ($picked | is-empty) {
        {action: quit}
    } else {
        {action: submit, selected: ($picked | get index)}
    }
    $result | to json --raw | save --force $output
}
