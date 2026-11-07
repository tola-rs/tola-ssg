#!/usr/bin/env nu
# Render release notes for one tag from the commits after the previous tag.

use ../lib/semver.nu [compare-versions, parse-version, tag-version]

const CATEGORIES = [
    {prefix: 'feat', title: 'Features'}
    {prefix: 'fix', title: 'Bug Fixes'}
    {prefix: 'refactor', title: 'Refactor'}
    {prefix: 'perf', title: 'Performance'}
]
const SKIPPED = ['release' 'ci' 'chore' 'docs' 'build' 'style' 'test' 'fmt']
const SUBJECT = '^(?<prefix>[A-Za-z]+)(?:\([^)]*\))?!?: *(?<body>.+)$'

# The greatest tag below this version, or null for the first release.
def previous-tag [tag: string] {
    let current = (parse-version (tag-version $tag))
    let below = (
        ^git tag --list 'v[0-9]*'
        | lines
        | each {|candidate|
            let version = (try { parse-version (tag-version $candidate) } catch { null })
            {tag: $candidate, version: $version}
        }
        | where {|candidate|
            if $candidate.version == null { false } else { (compare-versions $candidate.version $current) < 0 }
        }
    )
    let best = ($below | reduce --fold null {|candidate, best|
        if $best == null { $candidate } else if (compare-versions $candidate.version $best.version) >= 0 { $candidate } else { $best }
    })
    if $best == null { null } else { $best.tag }
}

def categorize [subject: string] {
    let match = ($subject | parse --regex $SUBJECT | get -o 0)
    if $match != null {
        let prefix = ($match.prefix | str lowercase)
        let category = ($CATEGORIES | where prefix == $prefix | get -o 0)
        if $category != null { return {title: $category.title, body: $match.body} }
        if $prefix in $SKIPPED { return null }
    }
    if ($subject | str lowercase | str starts-with 'release ') { return null }
    {title: 'Other Changes', body: $subject}
}

def render [previous: string, tag: string, repository: string, end: string] {
    let changes = (
        ^git log --cherry-pick --right-only '--pretty=format:%s' ($previous + '...' + $end)
        | lines
        | each {|subject| $subject | str trim }
        | where {|subject| $subject != '' }
        | each {|subject| categorize $subject }
        | where {|change| $change != null }
    )
    let sections = (
        ($CATEGORIES | get title | append 'Other Changes') | each {|title|
            let bodies = (
                $changes
                | where title == $title
                | each {|change| {key: ($change.body | str lowercase), body: $change.body} }
                | uniq-by key
                | get body
            )
            if ($bodies | is-empty) {
                []
            } else {
                [('### ' + $title)] ++ ($bodies | each {|body| '- ' + $body }) ++ ['']
            }
        } | flatten
    )
    let changelog = ('**Full Changelog**: https://github.com/' + $repository + '/compare/' + $previous + '...' + $tag)
    (["## What's Changed" ''] ++ $sections ++ [$changelog '']) | str join "\n"
}

def main [
    --tag: string
    --commit: string
    --repository: string
    --output: string
] {
    if $tag == null or $commit == null or $repository == null or $output == null {
        error make { msg: 'notes: --tag, --commit, --repository, and --output are required' }
    }
    let end = (^git rev-parse --verify --quiet $commit | str trim)
    if $end == '' { error make { msg: $"notes: unknown commit '($commit)'" } }
    let previous = (previous-tag $tag)
    let text = (if $previous == null {
        (^git log -1 --format=%B $end | str trim) + "\n"
    } else {
        render $previous $tag $repository $end
    })
    $text | save --force $output
}
