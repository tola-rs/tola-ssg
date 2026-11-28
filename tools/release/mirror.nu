#!/usr/bin/env nu
# Mirror a published GitHub release and update the default download.

use ../lib/archive.nu [file-sha256]
use ../lib/semver.nu [is-prerelease, tag-version]

const BUCKET = 's3://tola-releases'

def upload [path: path, key: string, cache: string] {
    ^aws s3 cp $path $"($BUCKET)/($key)" --cache-control $cache --no-progress
}

def main [tag: string, directory: path] {
    let version = (tag-version $tag)
    let repository = $env.GITHUB_REPOSITORY
    let endpoint = $"repos/($repository)/releases"
    let release = (^gh api $"($endpoint)/tags/($tag)" | from json)
    if $release.tag_name != $tag or $release.draft {
        error make {msg: $"($tag) must be published on GitHub before mirroring"}
    }

    let assets = $release.assets
    let names = ($assets | get name)
    if 'checksums.txt' not-in $names or ($names | length) != ($names | uniq | length) {
        error make {msg: $"($tag) must have one checksums.txt and distinct asset names"}
    }
    for asset in $assets {
        if $asset.state != 'uploaded' or $asset.name !~ '^[A-Za-z0-9][A-Za-z0-9._+-]*$' or ($asset.digest | default '') !~ '^sha256:[a-f0-9]{64}$' {
            error make {msg: $"GitHub has no verified upload for ($asset.name) in ($tag)"}
        }
    }

    let archives = ($directory | path join 'archives')
    mkdir $archives
    ^gh release download $tag --repo $repository --dir $archives
    let files = ($assets | each {|asset|
        let path = ($archives | path join $asset.name)
        let digest = (file-sha256 $path)
        if $asset.digest != $"sha256:($digest)" {
            error make {msg: $"SHA-256 mismatch for ($asset.name) in ($tag)"}
        }
        {name: $asset.name, path: $path, digest: $digest}
    })
    do {
        cd $archives
        ^sha256sum --check --strict checksums.txt
    }

    # Old and new archives can coexist when a GitHub release is rebuilt in place.
    # Installers select the archive by the digest in checksums.txt.
    for file in ($files | where name != 'checksums.txt') {
        upload $file.path $"($tag)/($file.digest)/($file.name)" 'public, max-age=31536000, immutable'
    }
    upload ($archives | path join 'checksums.txt') $"($tag)/checksums.txt" 'no-store'

    if $release.prerelease or (is-prerelease $version) { return }
    let latest = (^gh api $"($endpoint)/latest" | from json | get tag_name)
    if $latest != $tag { return }
    let marker = ($directory | path join 'latest.txt')
    $"($tag)\n" | save --force $marker
    upload $marker 'latest.txt' 'public, max-age=60'
}
