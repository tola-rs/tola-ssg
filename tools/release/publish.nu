#!/usr/bin/env nu
# Create or update verified GitHub release artifacts with an explicit publication mode.

use ../lib/repo.nu [check-tag, require-clean, release-mode]
use ../lib/semver.nu [is-prerelease, tag-version]

const API = 'https://api.github.com'
const UPLOADS = 'https://uploads.github.com'

def fail [message: string] {
    error make { msg: $"publish: ($message)" }
}

def github-headers [token: string] {
    {
        Authorization: $"Bearer ($token)"
        Accept: 'application/vnd.github+json'
    }
}

def github-list [url: string, token: string] {
    mut items = []
    mut page = 1
    loop {
        let batch = (http get $"($url)?per_page=100&page=($page)" --headers (github-headers $token))
        $items = ($items | append $batch)
        if ($batch | length) < 100 { return $items }
        $page += 1
    }
}

def release-assets [base: string, id: int, token: string] {
    github-list $"($base)/releases/($id)/assets" $token
}

def asset-matches [remote: record, asset: record] {
    ($remote.state == 'uploaded' and $remote.size == $asset.size and
        ($remote | get -o digest) == $"sha256:($asset.sha256)")
}

def verify-assets [tag: string, assets: list<record>, remote: list<record>, --allow-extra] {
    if not $allow_extra and ($remote | length) != ($assets | length) {
        fail $"release ($tag) has ($remote | length) assets, expected ($assets | length)"
    }
    for asset in $assets {
        let found = ($remote | where name == $asset.name)
        if ($found | length) != 1 { fail $"asset ($asset.name) is missing or duplicated" }
        if not (asset-matches ($found | first) $asset) {
            fail $"asset ($asset.name) differs or lacks a verified SHA-256"
        }
    }
}

def current-release [base: string, id: int, tag: string, token: string] {
    let release = (http get $"($base)/releases/($id)" --headers (github-headers $token))
    if $release.id != $id or $release.tag_name != $tag { fail 'release identity changed during publication' }
    if ($release | get -o immutable | default false) { fail $"release ($tag) is immutable" }
    $release
}

def verify-update [base: string, release: record, original: record, token: string] {
    current-release $base $release.id $release.tag_name $token | ignore
    let current = (http get $"($base)/git/ref/tags/($release.tag_name)" --headers (github-headers $token))
    if $current.object.type != $original.type or $current.object.sha != $original.sha {
        fail $"tag ($release.tag_name) moved during publication"
    }
}

def upload-asset [repository: string, id: int, asset: record, token: string] {
    let query = ({name: $asset.name} | url build-query)
    let upload = $"($UPLOADS)/repos/($repository)/releases/($id)/assets?($query)"
    let bytes = (open --raw $asset.path | into binary)
    http post $upload $bytes --headers (github-headers $token) --content-type 'application/octet-stream' | ignore
}

def verify-tag [base: string, tag: string, selected: string, token: string, --create] {
    for attempt in 1..3 {
        let response = (http get $"($base)/commits/tags/($tag)" --headers (github-headers $token) --full --allow-errors)
        if $response.status == 200 {
            if $response.body.sha != $selected {
                fail $"tag ($tag) points to ($response.body.sha), expected ($selected)"
            }
            return
        }
        if $response.status != 404 or $attempt == 3 {
            fail $"cannot verify tag ($tag): GitHub returned HTTP ($response.status)"
        }
        if $create and $attempt == 1 {
            let reference = {ref: $"refs/tags/($tag)", sha: $selected}
            http post $"($base)/git/refs" ($reference | to json) --headers (github-headers $token) --content-type 'application/json' | ignore
        }
        sleep 1sec
    }
}

def main [
    --tag: string
    --commit: string
    --repository: string
    --archives: string
    --notes: string
    --mode: string = 'create' # create, update-preserve-notes, or update-regenerate-notes
    --event: string # push or workflow_dispatch; updates require workflow_dispatch
] {
    release-mode $mode | ignore
    if $tag == null or $commit == null or $repository == null or $archives == null {
        fail '--tag, --commit, --repository, and --archives are required'
    }
    if $event not-in ['push' 'workflow_dispatch'] { fail '--event must be push or workflow_dispatch' }
    if $mode != 'create' and $event != 'workflow_dispatch' {
        fail 'release updates require workflow_dispatch'
    }
    if not ($repository =~ '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
        fail $"invalid repository: '($repository)'"
    }
    let version = (tag-version $tag)
    let root = ($env.FILE_PWD | path dirname | path dirname)
    require-clean $root
    let selected = (check-tag $root $tag $commit --mode $mode)
    let body = if $mode == 'update-preserve-notes' { '' } else {
        if $notes == null { fail '--notes is required for create and update-regenerate-notes' }
        if not ($notes | path exists) { fail $"notes file does not exist: ($notes)" }
        open --raw $notes
    }
    let assets = (
        ls $archives | where type == 'file' | each {|file|
            {
                name: ($file.name | path basename)
                path: $file.name
                size: ($file.size | into int)
                sha256: (open --raw $file.name | hash sha256)
            }
        }
    )
    if ($assets | is-empty) { fail $"no assets in ($archives)" }
    let token = ($env | get -o GH_TOKEN | default '')
    if $token == '' { fail 'GH_TOKEN is required' }

    let base = $"($API)/repos/($repository)"
    let selected_remote = (http get $"($base)/git/commits/($selected)" --headers (github-headers $token))
    if $selected_remote.sha != $selected { fail 'GitHub returned a different selected commit' }
    let branch = (http get $"($base)/git/ref/heads/($tag)" --headers (github-headers $token) --full --allow-errors)
    if $branch.status != 404 { fail $"release tag ($tag) conflicts with a branch or its absence could not be verified" }
    let releases = (github-list $"($base)/releases" $token | where tag_name == $tag)
    if ($releases | length) > 1 { fail $"multiple releases exist for ($tag)" }
    let existing = ($releases | get -o 0)

    if $mode != 'create' {
        if $existing == null { fail $"release ($tag) does not exist; use create mode" }
        let original = (http get $"($base)/git/ref/tags/($tag)" --headers (github-headers $token) | get object)
        let original_commit = (http get $"($base)/commits/tags/($tag)" --headers (github-headers $token) | get sha)
        verify-update $base $existing $original $token
        let remote = (release-assets $base $existing.id $token)
        for asset in $assets {
            let matching = ($remote | where name == $asset.name)
            if ($matching | length) > 1 { fail $"duplicate asset ($asset.name)" }
            let found = ($matching | get -o 0)
            if $found != null and (asset-matches $found $asset) { continue }
            if $found != null {
                verify-update $base $existing $original $token
                http delete $"($base)/releases/assets/($found.id)" --headers (github-headers $token) | ignore
            }
            verify-update $base $existing $original $token
            upload-asset $repository $existing.id $asset $token
        }
        verify-assets $tag $assets (release-assets $base $existing.id $token) --allow-extra
        verify-update $base $existing $original $token
        if $original_commit != $selected {
            let moved = (http patch $"($base)/git/refs/tags/($tag)" ({sha: $selected, force: true} | to json)
                --headers (github-headers $token) --content-type 'application/json')
            if $moved.object.type != 'commit' or $moved.object.sha != $selected {
                fail $"GitHub did not move ($tag) to ($selected)"
            }
        }
        verify-tag $base $tag $selected $token
        if $mode == 'update-regenerate-notes' {
            current-release $base $existing.id $tag $token | ignore
            (http patch $"($base)/releases/($existing.id)" ({name: $tag, body: $body} | to json)
                --headers (github-headers $token) --content-type 'application/json') | ignore
        }
        verify-tag $base $tag $selected $token
        print $existing.html_url
        return
    }

    if $existing != null and $existing.prerelease != (is-prerelease $version) {
        fail $"release ($tag) has a prerelease state that disagrees with its version"
    }
    if $existing != null and not $existing.draft {
        verify-tag $base $tag $selected $token
        verify-assets $tag $assets (release-assets $base $existing.id $token)
        print $existing.html_url
        return
    }
    let remote = if $existing == null { [] } else {
        current-release $base $existing.id $tag $token | ignore
        release-assets $base $existing.id $token
    }
    for asset in $assets {
        let matching = ($remote | where name == $asset.name)
        if ($matching | length) > 1 { fail $"duplicate asset ($asset.name)" }
        if ($matching | is-not-empty) and not (asset-matches ($matching | first) $asset) {
            fail $"asset ($asset.name) differs or lacks a verified SHA-256"
        }
    }
    if $event == 'workflow_dispatch' {
        verify-tag $base $tag $selected $token --create
    } else {
        verify-tag $base $tag $selected $token
    }
    let release = if $existing == null {
        http post $"($base)/releases" ({
            tag_name: $tag
            target_commitish: $selected
            name: $tag
            body: $body
            draft: true
            prerelease: (is-prerelease $version)
        } | to json) --headers (github-headers $token) --content-type 'application/json'
    } else { $existing }

    for asset in $assets {
        if ($remote | where name == $asset.name | is-not-empty) { continue }
        let current = (current-release $base $release.id $tag $token)
        if not $current.draft { fail $"release ($tag) is no longer a draft" }
        verify-tag $base $tag $selected $token
        upload-asset $repository $release.id $asset $token
    }
    verify-assets $tag $assets (release-assets $base $release.id $token)
    verify-tag $base $tag $selected $token
    let current = (current-release $base $release.id $tag $token)
    if not $current.draft { fail $"release ($tag) is no longer a draft" }
    if $existing != null {
        print $"Draft ($tag) has verified assets and remains unpublished."
        print $current.html_url
        return
    }
    let published = (
        http patch $"($base)/releases/($release.id)" ({draft: false} | to json)
            --headers (github-headers $token) --content-type 'application/json'
    )
    if $published.draft or $published.tag_name != $tag { fail $"GitHub did not publish ($tag)" }
    verify-tag $base $tag $selected $token
    print $published.html_url
}
