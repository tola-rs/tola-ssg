use tola ui

def main [input: path, output: path] {
    let data = (open --raw $input | from json)
    let bodies = $data.bodies
    let targets = $data.targets
    let tree = ($data.groups | reduce -f {} {|group, acc|
        $acc | insert $group.name $group.pages
    })
    let app = ($tree
        | tui label --titlebar $data.title
        | tui split --sizes ['35%' '1fr'] [
            (tui box Contents [
                (tui search --id search --bind / --placeholder 'Filter help')
                (tui tree --id tree --focus)
            ])
            (tui preview --id body --title Page --from tree {|node|
                if (($node | describe --detailed | get type) == 'string') {
                    $bodies | get $node
                } else {
                    'Select a page, then expand it with →.'
                }
            })
        ]
        | tui label --status '/: filter · ↑/↓: move · →/←: expand · Enter: read in pager · q: quit'
        | tui bind enter {|state|
            let picked = ($state.values.tree.value? | default null)
            if (($picked | describe --detailed | get type) == 'string') {
                {action: submit, selected: ($targets | get $picked)}
            }
        })
    let outcome = ($app | tui run)
    let picked = ($outcome.selected? | default null)
    let result = if $outcome.action == 'submit' and (($picked | describe --detailed | get type) == 'string') {
        {action: submit, selected: $picked}
    } else {
        {action: quit}
    }
    $result | to json --raw | save --force $output
}
