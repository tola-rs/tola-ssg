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
  --out: string = ""  # write the tree digest here
] {
  let os = $nu.os-info.name
  let bin = ($env.TOLA_BIN? | default (if $os == "windows" { "target/debug/tola.exe" } else { "target/debug/tola" }) | path expand)
  let out_path = (if (($out | str length) > 0) { $out | path expand } else { "" })
  let https_url = ($env.TOLA_FIXTURE_HTTPS_URL? | default "https://github.com/tola-rs/private-tola-module-for-ci-tests.git")
  let ssh_url = ($env.TOLA_FIXTURE_SSH_URL? | default "git@github.com:tola-rs/private-tola-module-for-ci-tests.git")
  let ssh_url_alt = ($env.TOLA_FIXTURE_SSH_URL_ALT? | default "ssh://git@github.com/tola-rs/private-tola-module-for-ci-tests.git")
  let public_url = ($env.TOLA_FIXTURE_PUBLIC_URL? | default "https://github.com/octocat/Hello-World.git")
  let tag = ($env.TOLA_FIXTURE_TAG? | default "v0.1.1")
  let subdir = ($env.TOLA_FIXTURE_SUBDIR? | default "theme")
  let token = ($env.TOLA_CI_HTTPS_TOKEN? | default "")
  let ssh_key = ($env.TOLA_CI_SSH_KEY? | default "")
  let known_hosts = ($env.TOLA_CI_SSH_KNOWN_HOSTS? | default "")

  let base = ($env.TMPDIR? | default ($env.TEMP? | default "/tmp"))
  let root = ($base | path join $"tola-private-module-(random uuid)")
  mkdir $root
  let ctx = {bin: $bin, root: $root, os: $os, token: $token, ssh_key: $ssh_key, known_hosts: $known_hosts}

  try {
    mut results = []

    if (($token | str length) > 0) {
      let tagged = (run-case $ctx "https-tag" $https_url $"tag = '($tag)'" "tag" $tag $subdir "update" "https")
      $results = ($results | append $tagged)
      $results = ($results | append (run-case $ctx "https-branch" $https_url "branch = 'main'" "branch" "main" $subdir "update" "https"))
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
    let digests = ($results | each {|result| $result.digest})
    if (($digests | uniq | length) > 1) { fail "private-module" $"transports produced different digests: ($digests)" }
    let digest = ($results | first | get digest)
    let first = ($results | first)

    expect-failure $ctx $first "--locked"
    expect-failure $ctx $first "--offline"
    expect-failure $ctx $first "--pure"
    print "flags: locked/offline/pure refused"
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

    if (($out_path | str length) > 0) { $digest | save --force $out_path }
    print $"tree_sha256: ($digest)"
  } catch {|err|
    print $"private-module checks failed: ($err.msg? | default 'unknown error')"
    rm -rf $root
    error make { msg: ($err.msg? | default "private-module checks failed") }
  }
  rm -rf $root
}

def fail [label, message] {
  error make { msg: $"($label): ($message)" }
}

def newline [] { char newline }
def tab [] { char tab }

def write-site [site, url, selector, subdir, vendor: bool = true] {
  mkdir $site
  mut lines = ["[build]", "entry = 'site.typ'", "", "[modules.ci]", $"git = '($url)'"]
  if (($selector | str length) > 0) { $lines = ($lines | append $selector) }
  if (($subdir | str length) > 0) { $lines = ($lines | append $"subdir = '($subdir)'") }
  if $vendor {
    $lines = ($lines | append "" "[vendor]" "path = 'vendor'")
  }
  (($lines | str join (newline))) | save --force ($site | path join "tola.toml")
  '#set document(title: "private-module")' | save --force ($site | path join "site.typ")
  mkdir ($site | path join "content")
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
  let config = ("[core]" + (newline) + (tab) + "autocrlf = true" + (newline) + (tab) + "eol = crlf" + (newline) + (tab) + "sshCommand = " + $ssh_command + (newline))
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
  let cache_file = ($site | path join ".tola" "modules" "ci" $module.commit $subdir "lib.typ")
  if not ($cache_file | path exists) { fail $label "cache file missing" }
  if ((open --raw $cache_file) | str contains $crlf) { fail $label "EOL pin did not hold" }

  let secret = ($ctx.token | str length) > 0
  if ($secret and ($lock_text | str contains $ctx.token)) { fail $label "token leaked into tola.lock" }
  let key_material = ($ctx.ssh_key | str length) > 0
  if ($key_material and ($lock_text | str contains $ctx.ssh_key)) { fail $label "private key leaked into tola.lock" }

  {label: $label, site: $site, home: $home, commit: $module.commit, digest: $module.tree_sha256}
}

def expect-failure [ctx, source, flag] {
  let result = (run-tola $ctx $source.site $source.home "update" $flag)
  if ($result.exit_code == 0) { fail "flags" $"update ($flag) unexpectedly succeeded" }
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
  let output = ($result.stdout + $result.stderr)
  if not (($output | str contains "fetch module") and ($output | str contains "ci")) { fail "bad credentials" "module alias missing from the diagnostic" }
}

def missing-repo-check [ctx, root, tag, subdir] {
  let home = ($root | path join "missing-repo-home")
  let site = ($root | path join "missing-repo-site")
  mkdir $home
  prepare-https $home "bad" $ctx.os
  write-site $site "https://github.com/tola-rs/definitely-no-such-private-module.git" $"tag = '($tag)'" $subdir
  let result = (run-tola $ctx $site $home "update")
  if ($result.exit_code == 0) { fail "missing repository" "unexpectedly succeeded" }
  let output = ($result.stdout + $result.stderr)
  if not (($output | str contains "fetch module") and ($output | str contains "ci")) { fail "missing repository" "module alias missing from the diagnostic" }
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
  let output = ($result.stdout + $result.stderr)
  if not ($output | str lowercase | str contains "ssh") { fail "no ssh" "ssh missing from the diagnostic" }
}
