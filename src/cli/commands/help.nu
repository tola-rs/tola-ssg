def node-page [data: record, node: any] {
    if $node == [] {
        $data.overview
    } else if (($node | describe --detailed | get type) == 'string') {
        $node
    } else {
        $data.groups | where {|group| $group.pages == $node } | get -o 0.name
    }
}

def browse [data: record, enter: string] {
    # An empty child list displays its key as a leaf in the tree.
    let roots = ({} | insert $data.overview [])
    let groups = ($data.groups | reduce -f $roots {|group, acc|
        $acc | insert $group.name $group.pages
    })
    let tree = if $data.last_page == null {
        $groups
    } else {
        let recent = if $data.language == 'zh-Hans' { '最近查看' } else { 'Recent' }
        {} | insert $recent $data.last_page | merge $groups
    }
    let chinese = ($data.language == 'zh-Hans')
    let contents = if $chinese { '目录' } else { 'Contents' }
    let preview = if $chinese { '正文' } else { 'Page' }
    let status = if $chinese {
        $'/: 搜索 · ↑/↓: 移动 · →/←: 展开或折叠 · Enter: ($enter) · q: 退出'
    } else {
        $'/: search · ↑/↓: move · →/←: expand/collapse · Enter: ($enter) · q: quit'
    }
    let app = ($tree
        | tui label --titlebar $data.title
        | tui split --sizes [$data.contents_columns '1fr'] [
            (tui tree --id tree --title $contents --focus)
            (tui preview --title $preview --from tree {|node|
                let page = (node-page $data $node)
                if $page != null { $data.bodies | get $page }
            })
        ]
        | tui label --status $status
        | tui bind / {|| {action: submit, selected: {search: true}}}
        | tui bind enter {|state|
            let page = (node-page $data $state.values.tree.value)
            if $page != null {
                {action: submit, selected: {page: $page, target: ($data.targets | get $page)}}
            }
        })
    let outcome = ($app | tui run)
    if $outcome.action == 'submit' { $outcome.selected }
}

def excerpt [text: string, query: string] {
    let index = ($text | str lowercase | str index-of --grapheme-clusters $query)
    let start = ([($index - 24) 0] | math max)
    let end = ($start + 96)
    let left = if $start > 0 { '…' } else { '' }
    let right = if ($text | str length --grapheme-clusters) > $end { '…' } else { '' }
    $'($left)($text | str substring --grapheme-clusters $start..<$end)($right)'
}

def matches [sections: list, raw: string] {
    let query = ($raw | str trim | str lowercase | str replace --all --regex '\s+' ' ')
    if ($query | is-empty) { return [] }
    let pattern = ($query | split chars | str escape-regex | str join '.*?')
    $sections | each {|row|
        let title = if $row.section == '' { $row.page } else { $row.section }
        let name = ($title | str lowercase)
        let paragraph = ($row.paragraphs | where {|text| $text | str contains --ignore-case $query } | first 1 | get -o 0)
        let score = if $name == $query or ($name | str starts-with $'($query) - ') {
            0
        } else if ($name | str contains $query) {
            1
        } else if $paragraph != null {
            2
        } else if $name =~ $'^($pattern)' {
            3
        } else if $name =~ $pattern {
            4
        } else {
            return null
        }
        let text = $paragraph | default ($row.paragraphs | get -o 0 | default '')
        {
            page: $row.page
            section: $row.section
            excerpt: (excerpt $text $query)
            target: $row.target
            score: $score
            width: (if $score >= 3 {
                $name | parse --regex ('(?<match>' + $pattern + ')') | get match.0 | str length
            } else {
                $title | str length
            })
            query: $raw
        }
    } | sort-by score width page section
}

def search [data: record, enter: string] {
    let chinese = ($data.language == 'zh-Hans')
    let title = if $chinese { '搜索帮助' } else { 'Search help' }
    let input_title = if $chinese { '搜索' } else { 'Search' }
    let placeholder = if $chinese { '搜索标题和正文' } else { 'Search titles and text' }
    let results_title = if $chinese { '结果' } else { 'Results' }
    let columns = if $chinese { [页面 小节 摘录] } else { [page section excerpt] }
    let status = if $chinese {
        $'Tab: 切到结果 · ↑/↓: 移动 · Enter: ($enter) · Ctrl+G: 返回'
    } else {
        $'Tab: focus results · ↑/↓: move · Enter: ($enter) · Ctrl+G: back'
    }
    mut query = ''
    loop {
        let app = (results $data.sections $query $chinese
            | tui label --titlebar $title
            | tui textbox --id query --title $input_title --value $query --focus --placeholder $placeholder
            | tui table --id results --title $results_title --columns $columns
            | tui label --status $status
            | tui bind ctrl+g {|| {action: quit}})
        let outcome = ($app | tui run --refresh 100ms {|state|
            let text = $state.values.query
            if ($state.values.results.row.query? | default null) != $text {
                results $data.sections $text $chinese
            }
        })
        if $outcome.action != 'submit' { return null }
        $query = $outcome.values.query
        let row = $outcome.values.results.row?
        let picked = if ($row.query? | default null) == $query {
            $row
        } else {
            matches $data.sections $query | get -o 0
        }
        if $picked.target? != null { return ($picked | select page target) }
    }
}

def results [sections: list, query: string, chinese: bool] {
    let rows = (matches $sections $query)
    let display = if ($rows | is-not-empty) { $rows } else {
        let message = if ($query | str trim | is-empty) { '' } else if $chinese { '没有匹配结果' } else { 'No matches' }
        [{page: '', section: '', excerpt: $message, query: $query}]
    }
    if $chinese {
        $display | each {|row| $row | merge {页面: $row.page, 小节: $row.section, 摘录: $row.excerpt} }
    } else {
        $display
    }
}

def main [input: path, output: path] {
    let data = (open --raw $input | from json)
    let enter = if $data.language == 'zh-Hans' {
        if $data.paged { '阅读' } else { '输出正文' }
    } else if $data.paged { 'read in pager' } else { 'print page' }
    mut result = {action: quit}
    loop {
        let choice = (browse $data $enter)
        if $choice == null { break }
        let picked = if ($choice.search? | default false) {
            search $data $enter
        } else {
            $choice
        }
        if $picked != null {
            $result = {action: submit, selected: $picked}
            break
        }
    }
    $result | to json --raw | save --force $output
}
