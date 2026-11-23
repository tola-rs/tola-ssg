use tola get-hook

def html-digests [directory: string] {
    cd $directory
    glob '**/*.html' --no-dir | sort | each {|path|
        [($path | path relative-to $directory) (open --raw $path | hash sha256)]
    }
}

# A direct executable can provide the stock stylesheet itself. A launcher or
# custom stylesheet may load other dependencies identified by `toolchain`.
export def cache-key [context: record, argv: list<string>, stylesheet: string] {
    let options = $context.module.options
    if not $options.cache { return null }
    let custom = ($stylesheet | path exists)
    let pinned = ($options.toolchain | str trim) != ''
    if not $pinned and ($options.runner != 'auto' or $custom) { return null }
    let program = (open --raw ($argv | first) | into binary)
    if not $pinned {
        let magic = ($program | bytes at 0..<4 | encode hex | str lowercase)
        if $magic !~ '^(7f454c46|4d5a|feedfac[ef]|c[ef]faedfe|cafebab[ef]|b[ef]bafeca)' { return null }
    }
    let watched = (get-hook --rerun-on-digest)
    if $watched == null { return null }
    {
        scripts: ([compile.nu cache.nu] | each {|file|
            open --raw ($context.module.root | path join $file) | hash sha256
        })
        argv: $argv
        executable: ($program | hash sha256)
        input-scope: $context.inputScope
        node-env: $env.NODE_ENV?
        toolchain: $options.toolchain
        minify: $options.minify
        stylesheet: (if $custom { open --raw $stylesheet | hash sha256 })
        html: (html-digests $context.hook.inputDir)
        watched: $watched
    } | to json --raw | hash sha256
}

export def read-css [directory: string, key: string] {
    let stored = (open ($directory | path join $"($key).json"))
    let css = ($stored.css | decode base64)
    if ($css | hash sha256) == $stored.digest { $css }
}

export def store-css [directory: string, key: string, output: string] {
    let temporary = ($directory | path join $"(random uuid).tmp")
    try {
        let css = (open --raw $output | into binary)
        {css: ($css | encode base64), digest: ($css | hash sha256)} | to json --raw | save --raw $temporary
        mv --force $temporary ($directory | path join $"($key).json")
    } catch {
        if ($temporary | path exists) { try { rm $temporary } }
    }
}
