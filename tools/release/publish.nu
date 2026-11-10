#!/usr/bin/env nu
# Create one draft GitHub release from local archives, verify it, then publish it.

use ../lib/repo.nu [check-tag, require-clean]
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

def release-assets [base: string, id: int, token: string] {
    http get $"($base)/releases/($id)/assets?per_page=100" --headers (github-headers $token)
}

def main [
    --tag: string
    --commit: string
    --repository: string
    --archives: string
    --notes: string
] {
    if $tag == null or $commit == null or $repository == null or $archives == null or $notes == null {
        fail '--tag, --commit, --repository, --archives, and --notes are required'
    }
    if not ($repository =~ '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
        fail $"invalid repository: '($repository)'"
    }
    let version = (tag-version $tag)
    let root = ($env.FILE_PWD | path dirname | path dirname)
    require-clean $root
    let selected = (check-tag $root $tag $commit)
    if not ($notes | path exists) { fail $"notes file does not exist: ($notes)" }
    let body = (open --raw $notes)
    let assets = (
        ls $archives
        | where type == 'file'
        | each {|file|
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
    let releases = (
        http get $"($base)/releases?per_page=100" --headers (github-headers $token)
        | where tag_name == $tag
    )
    if ($releases | length) > 1 { fail $"multiple releases exist for ($tag)" }
    let existing = ($releases | get -o 0)
    if $existing != null {
        if not ($existing.draft) { fail $"release ($tag) is already published" }
    }
    let release = (if $existing == null {
        http post $"($base)/releases" ({
            tag_name: $tag
            target_commitish: $selected
            name: $tag
            body: $body
            draft: true
            prerelease: (is-prerelease $version)
        } | to json) --headers (github-headers $token) --content-type 'application/json'
    } else {
        $existing
    })

    let release_url = $"($base)/releases/($release.id)"
    let remote = (release-assets $base $release.id $token)
    if $release.target_commitish != $selected {
        if ($remote | is-not-empty) {
            fail $"draft ($tag) already has assets for ($release.target_commitish), expected ($selected)"
        }
        let updated = (http patch $release_url ({target_commitish: $selected, body: $body} | to json)
            --headers (github-headers $token) --content-type 'application/json')
        if $updated.target_commitish != $selected or $updated.draft != true {
            fail $"could not set draft ($tag) to ($selected)"
        }
    }
    for asset in $assets {
        let found = ($remote | where name == $asset.name | get -o 0)
        if $found == null {
            let query = ({name: $asset.name} | url build-query)
            let upload = $"($UPLOADS)/repos/($repository)/releases/($release.id)/assets?($query)"
            let bytes = (open --raw $asset.path | into binary)
            http post $upload $bytes --headers (github-headers $token) --content-type 'application/octet-stream' | ignore
        } else if $found.size != $asset.size {
            fail $"remote ($asset.name) is ($found.size) bytes, expected ($asset.size)"
        }
    }

    let final = (release-assets $base $release.id $token)
    if ($final | length) != ($assets | length) {
        fail $"release ($tag) has ($final | length) assets, expected ($assets | length)"
    }
    for asset in $assets {
        let found = ($final | where name == $asset.name | get -o 0)
        if $found == null { fail $"asset ($asset.name) is missing" }
        if $found.size != $asset.size { fail $"asset ($asset.name) is ($found.size) bytes, expected ($asset.size)" }
        let digest = ($found | get -o digest)
        if $digest != null and $digest != $"sha256:($asset.sha256)" {
            fail $"asset ($asset.name) has digest ($digest), expected sha256:($asset.sha256)"
        }
    }

    let published = (
        http patch $release_url ({draft: false} | to json) --headers (github-headers $token) --content-type 'application/json'
    )
    if $published.draft != false { fail $"release ($tag) is still a draft" }
    print $published.html_url
}
