#!/usr/bin/env nu
# Private-module checks: HTTPS through a user credential helper, SSH through a
# read-only deploy key, selectors, subdir, EOL pinning, flags, vendor, and failure
# cases. Secrets arrive through the environment, never through argv, and are never
# printed or written to the lock.
#
#   TOLA_CI_HTTPS_TOKEN       optional; enables the HTTPS cases
#   TOLA_CI_SSH_KEY           private key content; enables the SSH cases
#   TOLA_CI_SSH_KNOWN_HOSTS   known_hosts content
#   TOLA_BIN                  tola binary (default target/debug/tola[.exe])
#   TOLA_FIXTURE_*            fixture coordinates; see the defaults below

def main [
  --out: string = ""  # write the digest of each tested selector here
] {
  let os = $nu.os-info.name
  let bin = ($env.TOLA_BIN? | default (if $os == "windows" { "target/debug/tola.exe" } else { "target/debug/tola" }) | path expand)
  let out_path = (if (($out | str length) > 0) { $out | path expand } else { "" })
  let https_url = ($env.TOLA_FIXTURE_HTTPS_URL? | default "https://github.com/tola-rs/private-tola-module-for-ci-tests.git")
  let ssh_url = ($env.TOLA_FIXTURE_SSH_URL? | default "git@github.com:tola-rs/private-tola-module-for-ci-tests.git")
  let ssh_url_alt = ($env.TOLA_FIXTURE_SSH_URL_ALT? | default "ssh://git@github.com/tola-rs/private-tola-module-for-ci-tests.git")
  let public_url = ($env.TOLA_FIXTURE_PUBLIC_URL? | default "https://github.com/octocat/Hello-World.git")
  let tag = ($env.TOLA_FIXTURE_TAG? | default "v0.2.1")
  let branch = ($env.TOLA_FIXTURE_BRANCH? | default "codex/module-fixture")
  let subdir = ($env.TOLA_FIXTURE_SUBDIR? | default "theme")
  let token = ($env.TOLA_CI_HTTPS_TOKEN? | default "")
  let ssh_key = ($env.TOLA_CI_SSH_KEY? | default "")
  let known_hosts = ($env.TOLA_CI_SSH_KNOWN_HOSTS? | default "")

  let root = (mktemp --directory)
  let ctx = {bin: $bin, root: $root, os: $os, token: $token, ssh_key: $ssh_key, known_hosts: $known_hosts}

  try {
    mut results = []

    if (($token | str length) > 0) {
      let tagged = (run-case $ctx "https-tag" $https_url $"tag = '($tag)'" "tag" $tag $subdir "update" "https")
      $results = ($results | append $tagged)
      $results = ($results | append (run-case $ctx "https-branch" $https_url $"branch = '($branch)'" "branch" $branch $subdir "update" "https"))
      $results = ($results | append (run-case $ctx "https-rev" $https_url $"rev = '($tagged.commit)'" "rev" $tagged.commit $subdir "build" "https"))
      $results = ($results | append (run-case $ctx "https-default" $https_url "" "none" "" $subdir "update" "https"))
    } else {
      print "https: skipped (TOLA_CI_HTTPS_TOKEN is empty)"
    }

    let has_key = (($ssh_key | str length) > 0)
    let has_hosts = (($known_hosts | str length) > 0)
    if ($has_key and $has_hosts) {
      $results = ($results | append (run-case $ctx "ssh-scp" $ssh_url $"tag = '($tag)'" "tag" $tag $subdir "update" "ssh"))
      $results = ($results | append (run-case $ctx "ssh-url" $ssh_url_alt $"tag = '($tag)'" "tag" $tag $subdir "update" "ssh"))
    } else {
      print "ssh: skipped (TOLA_CI_SSH_KEY / TOLA_CI_SSH_KNOWN_HOSTS is empty)"
    }

    for result in $results { print $"($result.label) ok: ($result.digest)" }
    if (($results | length) == 0) { fail "private-module" "no transport was tested" }
    let tagged = ($results | where kind != "branch")
    if (($tagged | get digest | uniq | length) != 1) { fail "private-module" "tag, rev, default, and SSH selections disagree" }
    let branches = ($results | where kind == "branch")
    if not ($branches | is-empty) {
      if (($branches | first | get digest) == ($tagged | first | get digest)) { fail "private-module" "branch did not select its distinct module contents" }
    }
    let first = ($results | first)
    let digests = (($results | sort-by label | each {|result| $"($result.label) ($result.digest)"} | str join (newline)) + (newline))

    expect-failure $ctx $first "--locked"
    expect-failure $ctx $first "--offline"
    expect-failure $ctx $first "--pure"
    print "flags: locked/offline/pure refused"
    pack-check $ctx $first
    print "pack: generated release, replacement, failed preparation, and consumer build passed"
    cache-check $ctx $first
    print "cache: selected subtree, alias reuse, and locked-commit restoration passed"
    vendor-check $ctx $first
    print "vendor: pure build and tamper check passed"

    bad-credentials-check $ctx $root $https_url $tag $subdir
    print "negative: bad credentials rejected"
    missing-repo-check $ctx $root $tag $subdir
    print "negative: missing repository rejected"
    no-git-check $ctx $root $public_url
    print "negative: public fetch works without git"
    no-ssh-check $ctx $root $ssh_url $tag $subdir
    print "negative: missing ssh rejected"

    if (($out_path | str length) > 0) { $digests | save --force $out_path }
    print $digests
  } finally {
    rm -rf $root
  }
}

def fail [label, message] {
  error make { msg: $"($label): ($message)" }
}

def newline [] { char newline }
def tab [] { char tab }

def write-site [site, url, selector, subdir, vendor: bool = true] {
  mkdir $site
  mut lines = ["[build]", "entry = 'site.typ'", "", "[modules.ci.git]", $"url = '($url)'"]
  if (($selector | str length) > 0) { $lines = ($lines | append $selector) }
  if (($subdir | str length) > 0) { $lines = ($lines | append $"subdir = '($subdir)'") }
  if $vendor {
    $lines = ($lines | append "" "[vendor]" "path = 'vendor'")
  }
  (($lines | str join (newline))) | save --force ($site | path join "tola.toml")
  write-entry $site
}

def write-entry [site] {
  mkdir ($site | path join "content")
  ['#import "@tola-modules/ci:0.0.0": greeting', '#document("index.html", format: "html")[#greeting()]']
    | str join (newline)
    | save --force ($site | path join "site.typ")
}

def prepare-https [home, token, os] {
  let config = ("[credential]" + (newline) + (tab) + "helper = store" + (newline) + "[core]" + (newline) + (tab) + "autocrlf = true" + (newline) + (tab) + "eol = crlf" + (newline))
  $config | save --force ($home | path join ".gitconfig")
  ('https://x-access-token:' + $token + '@github.com' + (newline)) | save --force ($home | path join ".git-credentials")
  if $os != "windows" { ^chmod 600 ($home | path join ".git-credentials") }
}

def prepare-ssh [home, ssh_key, known_hosts, os] {
  let ssh_dir = ($home | path join ".ssh")
  mkdir $ssh_dir
  let key_path = ($ssh_dir | path join "id_ed25519")
  ($ssh_key + (newline)) | save --force $key_path
  if $os != "windows" { ^chmod 600 $key_path }
  let known_path = ($ssh_dir | path join "known_hosts")
  ($known_hosts + (newline)) | save --force $known_path
  let ssh_command = ('ssh -i "' + $key_path + '" -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile="' + $known_path + '"')
  let config = ("[core]" + (newline) + (tab) + "autocrlf = true" + (newline) + (tab) + "eol = crlf" + (newline) + (tab) + "sshCommand = " + ($ssh_command | to json --raw) + (newline))
  $config | save --force ($home | path join ".gitconfig")
}

def run-tola [ctx, site, home, action, flag: string = "", extra_env: record = {}] {
  let base = {HOME: $home, USERPROFILE: $home, GIT_TERMINAL_PROMPT: "0", GCM_INTERACTIVE: "never", GIT_CONFIG_NOSYSTEM: "1"}
  let merged = ($base | merge $extra_env)
  let args = if (($flag | str length) > 0) { [$action $flag] } else { [$action] }
  with-env $merged {
    cd $site
    run-external $ctx.bin ...$args | complete
  }
}

def check-revision [label, module, kind, value] {
  if $kind == "tag" { if ($module.revision.tag != $value) { fail $label "tag mismatch" } }
  if $kind == "branch" { if ($module.revision.branch != $value) { fail $label "branch mismatch" } }
  if $kind == "rev" { if ($module.revision.rev != $value) { fail $label "rev mismatch" } }
  if $kind == "none" { if (($module.revision? | default null) != null) { fail $label "default branch recorded a selector" } }
}

def run-case [ctx, label, url, selector, kind, value, subdir, action, transport] {
  let home = ($ctx.root | path join $"($label)-home")
  let site = ($ctx.root | path join $"($label)-site")
  mkdir $home
  mkdir $site
  if $transport == "https" { prepare-https $home $ctx.token $ctx.os } else { prepare-ssh $home $ctx.ssh_key $ctx.known_hosts $ctx.os }
  write-site $site $url $selector $subdir

  let result = (run-tola $ctx $site $home $action)
  if ($result.exit_code != 0) {
    print $result.stdout
    print $result.stderr
    fail $label $"tola ($action) failed"
  }

  let lock_text = (open --raw ($site | path join "tola.lock"))
  let lock = ($lock_text | from toml)
  let module = $lock.modules.ci
  if (($module.commit | str length) != 40) { fail $label "commit is not a full object id" }
  if (($module.tree_sha256 | str length) != 64) { fail $label "tree_sha256 is not 64 hex digits" }
  if ($module.subdir != $subdir) { fail $label "subdir was not recorded" }
  check-revision $label $module $kind $value

  let crlf = ((char cr) + (char lf))
  let cache = ($site | path join ".tola" "modules" $module.commit ($subdir | hash sha256))
  let cache_file = ($cache | path join "lib.typ")
  if not ($cache_file | path exists) { fail $label "cache file missing" }
  if ((open --raw $cache_file) | str contains $crlf) { fail $label "EOL pin did not hold" }

  let expected = if $kind == "branch" { "branch" } else { "tag" }
  if ((open --raw ($cache | path join "revision.txt") | str trim) != $expected) { fail $label "wrong module revision was fetched" }
  if ($cache | path join "tola.toml" | path exists) { fail $label "site files leaked into the module cache" }
  if ($cache | path join "theme" | path exists) { fail $label "cache contains the repository instead of its selected module" }
  if $action != "build" {
    let build = (run-tola $ctx $site $home "build")
    if ($build.exit_code != 0) { print $build.stderr; fail $label "module consumer build failed" }
  }
  if not ($site | path join "public" "index.html" | path exists) { fail $label "module consumer produced no page" }

  let secret = ($ctx.token | str length) > 0
  if ($secret and ($lock_text | str contains $ctx.token)) { fail $label "token leaked into tola.lock" }
  let key_material = ($ctx.ssh_key | str length) > 0
  if ($key_material and ($lock_text | str contains $ctx.ssh_key)) { fail $label "private key leaked into tola.lock" }

  {label: $label, kind: $kind, site: $site, home: $home, cache: $cache, commit: $module.commit, digest: $module.tree_sha256}
}

def expect-failure [ctx, source, flag] {
  let result = (run-tola $ctx $source.site $source.home "update" $flag)
  if ($result.exit_code == 0) { fail "flags" $"update ($flag) unexpectedly succeeded" }
}

def pack-check [ctx, selected] {
  let source = ($ctx.root | path join "pack-source")
  let release = ($ctx.root | path join "pack-release")
  let args = ["module" "pack" $source "--output" $release]
  cp -r $selected.cache $source
  rm -rf ($source | path join "assets")
  let packed = (run-external $ctx.bin ...$args | complete)
  if ($packed.exit_code != 0) { print $packed.stderr; fail "pack" "release preparation failed" }
  let manifest = (open ($release | path join "tola-module.toml"))
  if ("pack" in $manifest) { fail "pack" "author selection leaked into the release" }
  if ("before-pack" in ($manifest.hooks? | default {})) { fail "pack" "author hook leaked into the release" }
  if ($release | path join "author" | path exists) { fail "pack" "author scripts leaked into the release" }
  if not ($release | path join "assets" "theme.css" | path exists) { fail "pack" "before-pack did not generate the selected stylesheet" }
  "stale" | save ($release | path join "stale.txt")
  let replaced = (run-external $ctx.bin ...$args | complete)
  if ($replaced.exit_code != 0) { print $replaced.stderr; fail "pack" "release replacement failed" }
  if ($release | path join "stale.txt" | path exists) { fail "pack" "replacement retained a stale file" }
  rm ($source | path join "lib.typ")
  let failed = (run-external $ctx.bin ...$args | complete)
  if ($failed.exit_code == 0) { fail "pack" "a release missing its entry was accepted" }
  let checked = (run-external $ctx.bin module check $release | complete)
  if ($checked.exit_code != 0) { print $checked.stderr; fail "pack" "failed preparation damaged the previous release" }
  let consumer = ($ctx.root | path join "release-consumer")
  mkdir $consumer
  {build: {entry: "site.typ"}, modules: {ci: {path: $release}}} | to toml | save ($consumer | path join "tola.toml")
  write-entry $consumer
  let build = (run-tola $ctx $consumer $selected.home "build")
  if ($build.exit_code != 0) { print $build.stderr; fail "pack" "release consumer build failed" }
}

def cache-check [ctx, selected] {
  let config_path = ($selected.site | path join "tola.toml")
  let config = (open $config_path)
  $config | upsert modules.alias ($config.modules.ci | merge {assets: {enable: false}}) | to toml | save --force $config_path
  let reused = (run-tola $ctx $selected.site $selected.home "build")
  if ($reused.exit_code != 0) { print $reused.stderr; fail "cache" "second alias failed" }
  let roots = (glob ($selected.site | path join ".tola" "modules" "*" "*" "tola-module.toml"))
  if (($roots | length) != 1) { fail "cache" "aliases retained separate copies of one module" }
  let lock = (open --raw ($selected.site | path join "tola.lock"))
  rm -rf $selected.cache
  let offline = (run-tola $ctx $selected.site $selected.home "build" "--offline")
  if ($offline.exit_code == 0) { fail "cache" "offline build restored missing remote bytes" }
  let restored = (run-tola $ctx $selected.site $selected.home "build" "--locked")
  if ($restored.exit_code != 0) { print $restored.stderr; fail "cache" "locked module cache was not restored" }
  if ((open --raw ($selected.site | path join "tola.lock")) != $lock) { fail "cache" "restoring the cache changed the lock" }
}

def vendor-check [ctx, source] {
  let listed = (run-tola $ctx $source.site $source.home "vendor")
  if ($listed.exit_code != 0) { print $listed.stderr; fail "vendor" "tola vendor failed" }

  let build = (run-tola $ctx $source.site $source.home "build" "--pure")
  if ($build.exit_code != 0) { print $build.stderr; fail "vendor" "pure build failed" }

  let vendor = ($source.site | path join "vendor" "modules" "ci")
  if not ($vendor | path exists) { fail "vendor" "vendor module missing" }
  let files = (glob ($vendor | path join "**" "lib.typ"))
  if (($files | length) == 0) { fail "vendor" "vendor lib.typ missing" }
  let file = ($files | first)
  ((open --raw $file) + "tampered" + (newline)) | save --force $file
  let tampered = (run-tola $ctx $source.site $source.home "build" "--pure")
  if ($tampered.exit_code == 0) { fail "vendor" "tampered vendor passed the digest" }
}

def bad-credentials-check [ctx, root, url, tag, subdir] {
  let home = ($root | path join "bad-creds-home")
  let site = ($root | path join "bad-creds-site")
  mkdir $home
  prepare-https $home "bad" $ctx.os
  write-site $site $url $"tag = '($tag)'" $subdir
  let result = (run-tola $ctx $site $home "update")
  if ($result.exit_code == 0) { fail "bad credentials" "unexpectedly succeeded" }
}

def missing-repo-check [ctx, root, tag, subdir] {
  let home = ($root | path join "missing-repo-home")
  let site = ($root | path join "missing-repo-site")
  mkdir $home
  prepare-https $home "bad" $ctx.os
  write-site $site "https://github.com/tola-rs/definitely-no-such-private-module.git" $"tag = '($tag)'" $subdir
  let result = (run-tola $ctx $site $home "update")
  if ($result.exit_code == 0) { fail "missing repository" "unexpectedly succeeded" }
}

def no-git-check [ctx, root, public_url] {
  let home = ($root | path join "no-git-home")
  let site = ($root | path join "no-git-site")
  let path = ($root | path join "no-git-bin")
  mkdir $home
  mkdir $path
  write-site $site $public_url "" "" false
  let result = (run-tola $ctx $site $home "update" "" {PATH: $path})
  if ($result.exit_code != 0) { print $result.stderr; fail "no git" "public fetch failed" }
  if not (($site | path join "tola.lock") | path exists) { fail "no git" "public fetch wrote no lock" }
}

def no-ssh-check [ctx, root, url, tag, subdir] {
  let home = ($root | path join "no-ssh-home")
  let site = ($root | path join "no-ssh-site")
  let path = ($root | path join "no-ssh-bin")
  mkdir $home
  mkdir $path
  ("[core]" + (newline) + (tab) + "autocrlf = true" + (newline) + (tab) + "eol = crlf" + (newline)) | save --force ($home | path join ".gitconfig")
  write-site $site $url $"tag = '($tag)'" $subdir
  let result = (run-tola $ctx $site $home "update" "" {PATH: $path})
  if ($result.exit_code == 0) { fail "no ssh" "unexpectedly succeeded" }
}
