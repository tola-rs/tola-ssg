use tola [diagnostic get-context]
use ./cache.nu [cache-key read-css store-css]

def is-tailwind-cli [arg: string] {
    let name = ($arg | path basename | str lowercase)
    $name in [tailwindcss tailwindcss.exe tailwindcss.cmd tailwindcss.bat tailwindcss.ps1] or $arg =~ '^@tailwindcss/cli(@.+)?$'
}

def runner-issues [argv: list<string>] {
    let tailwind_start = ($argv | enumerate | where {|part|
        $part.item == '--' or (is-tailwind-cli $part.item)
    } | get --optional 0.index | default ($argv | length))

    $argv | enumerate | skip 1 | each {|part|
        let tailwind = $part.index > $tailwind_start
        let flag = ($part.item | split row '=' | first)
        let short = if $tailwind and $flag =~ '^-[a-zA-Z]+$' { $flag } else { '' }
        let hint = if $flag in ['--minify' '--no-minify'] or $short =~ 'm' {
            'Set minify = true or minify = false.'
        } else if $flag in ['--dev' '--no-dev'] {
            'Set dev to "run", "reuse", or "skip".'
        } else if $flag in ['--watch' '--no-watch' '--poll' '--no-poll'] or $short =~ 'w' {
            'Use tola dev to watch the site.'
        } else if $tailwind and ($flag in ['--input' '--output' '--cwd' '--map' '--no-map'] or $short =~ '[io]') {
            'Let the module supply the Tailwind input, output, and scan directory.'
        }
        if $hint != null {
            {key: 'runner', reason: $"contains the managed flag `($part.item)`", hint: $hint}
        }
    }
}

def check-options [options: record] {
    let fields = [[key type expected];
        [minify bool 'true or false']
        [cache bool 'true or false']
        [toolchain string 'a version or digest string']
    ]
    let scalar_issues = ($fields | each {|field|
        if ($options | get $field.key | describe) != $field.type {
            {key: $field.key, reason: $"must be ($field.expected)", hint: null}
        }
    })
    let runner = $options.runner
    let runner_issues = if $runner == 'auto' {
        []
    } else if not (($runner | describe) | str starts-with 'list<') or ($runner | is-empty) {
        [{key: 'runner', reason: 'must be "auto" or a nonempty command array', hint: 'Try runner = ["deno", "task", "tailwind"].'}]
    } else if not ($runner | all {|arg| ($arg | describe) == 'string' }) or (($runner | first | str trim) | is-empty) {
        [{key: 'runner', reason: 'must start with a command and contain only strings', hint: 'Write each argument as its own array element.'}]
    } else {
        runner-issues $runner
    }
    $scalar_issues | append $runner_issues
}

def executable [name: string] {
    which $"^($name)" | get --optional 0.path
}

def tailwind-argv [runner: any, site: string] {
    if $runner == 'auto' {
        let local = (executable ($site | path join 'node_modules' '.bin' 'tailwindcss'))
        let program = if $local != null { $local } else { executable tailwindcss }
        if $program != null { [$program] }
    } else {
        let program = (executable ($runner | first))
        if $program != null { $runner | update 0 $program }
    }
}

def css-path [path: string, from: string] {
    $path | path relative-to --walk-up $from | path split | str join '/'
}

def compile-css [context: record, argv: list<string>, stylesheet: string, output: string] {
    let hook = $context.hook
    let input = ($hook.tempDir | path join 'tailwind.css')
    let scan = ($hook.tempDir | path join 'scan')
    let entry = if ($stylesheet | path exists) { css-path $stylesheet $hook.tempDir } else { 'tailwindcss' }
    let pages = (css-path $hook.inputDir $hook.tempDir)
    let import = $'@import "($entry)"'
    mkdir $scan
    [
        ($import + ' source(none);')
        $'@source "($pages)/**/*.html";'
    ] | str join (char nl) | save --raw $input

    let flags = ['--input' $input '--output' $output '--cwd' $scan]
    let minify = if $context.module.options.minify { ['--minify'] } else { [] }
    let run = (try { run-external ...$argv ...$flags ...$minify | complete })
    if $run == null {
        let key = $"modules.($context.module.name).options.runner"
        diagnostic make 'Could not start Tailwind' --severity error --code tailwind-runner --hints [
            $"Check the command in `($key)` and its interpreter."
        ]
    } else if $run.exit_code != 0 {
        let stderr = ($run.stderr? | default '' | ansi strip | str trim | str substring 0..<4000)
        let notes = [$stderr] | where {|note| not ($note | is-empty) }
        diagnostic make $"Tailwind exited with status ($run.exit_code) while compiling the stylesheet" --severity error --code tailwind-compile --notes $notes
    } else if ($output | path type) != 'file' {
        diagnostic make 'The Tailwind runner did not write its CSS file' --severity error --code tailwind-output --hints [
            'Forward the module’s Tailwind arguments and exit after one build.'
        ]
    }
}

def compile-hook [context: record, argv: list<string>] {
    let hook = $context.hook
    let stylesheet = ($context.site.root | path join styles tailwind.css)
    let output = ($hook.outputDir | path join $hook.outputs.0.file)
    mkdir ($output | path dirname)

    let key = (try { cache-key $context $argv $stylesheet })
    let css = if $key != null { try { read-css $hook.cacheDir $key } }
    if $css != null {
        $css | save --raw $output
        return
    }
    let issue = (compile-css $context $argv $stylesheet $output)
    if $issue != null { return $issue }
    if $key != null and $key == (try { cache-key $context $argv $stylesheet }) {
        store-css $hook.cacheDir $key $output
    }
}

def run-tailwind [context: record] {
    let runner = $context.module.options.runner
    let argv = (tailwind-argv $runner $context.site.root)
    if $argv == null {
        let missing = if $runner == 'auto' { 'Tailwind in node_modules/.bin or on PATH' } else { $runner | first }
        diagnostic make $"Could not find ($missing)" --severity error --code tailwind-runner --hints [
            'Install Tailwind CSS v4 or set the module’s runner option.'
        ]
    } else {
        compile-hook $context $argv
    }
}

def main [] {
    let context = (get-context)
    let issues = (check-options $context.module.options | each {|issue|
        let key = $"modules.($context.module.name).options.($issue.key)"
        diagnostic make $"`($key)` ($issue.reason)" --severity error --code tailwind-option --hints ([$issue.hint] | compact)
    })
    let report = if ($issues | is-empty) {
        let issue = (run-tailwind $context)
        [$issue] | compact
    } else { $issues }
    $report | diagnostic submit
}
