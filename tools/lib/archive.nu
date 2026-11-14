# Release archives: one executable at the archive root, extraction, and file digests.

# Git Bash can put GNU tar first on PATH. Use Windows' ZIP-capable tar there.
def run-tar [archive: string, flags: string, ...operands: string]: nothing -> string {
    let tar = if $nu.os-info.name == 'windows' {
        $env.SystemRoot | path join 'System32' 'tar.exe'
    } else {
        'tar'
    }
    let result = (run-external $tar $flags $archive ...$operands | complete)
    if $result.exit_code != 0 {
        error make { msg: $"($archive | path basename): tar failed \(($result.exit_code)\): ($result.stderr | str trim)" }
    }
    $result.stdout
}

export def archive-members [archive: string]: nothing -> list<string> {
    if (use-unzip $archive) {
        run-unzip $archive ['-Z' '-1' $archive] | lines
    } else {
        run-tar $archive '-tf' | lines
    }
}

def use-unzip [archive: string]: nothing -> bool {
    $nu.os-info.name != 'windows' and ($archive | str ends-with '.zip')
}

def run-unzip [archive: string, args: list<string>]: nothing -> string {
    let result = (^unzip ...$args | complete)
    if $result.exit_code != 0 {
        error make { msg: $"($archive | path basename): unzip failed \(($result.exit_code)\): (($result.stderr + $result.stdout) | str trim)" }
    }
    $result.stdout
}

export def create-tar-gz [archive: string, directory: string, members: list<string>]: nothing -> nothing {
    run-tar $archive '-czf' '-C' $directory ...$members | ignore
}

# Write source as the only file in archive, named after the target's binary.
export def create-archive [source: string, archive: string, target: record] {
    let binary = ($target | get binary)
    if ($source | path type) != 'file' {
        error make { msg: $"release binary is not a regular file: ($source)" }
    }
    let stage = (mktemp --directory)
    try {
        cp $source ($stage | path join $binary)
        if ($target | get system) == 'windows' {
            if (use-unzip $archive) {
                let result = (do { cd $stage; ^zip -q $archive $binary } | complete)
                if $result.exit_code != 0 {
                    error make { msg: $"($archive | path basename): zip failed \(($result.exit_code)\): (($result.stderr + $result.stdout) | str trim)" }
                }
            } else {
                run-tar $archive '-acf' '-C' $stage $binary | ignore
            }
        } else {
            create-tar-gz $archive $stage [$binary]
        }
    } finally {
        rm --recursive --force $stage
    }
}

export def check-archive [archive: string, binary: string] {
    if (archive-members $archive) != [$binary] {
        error make { msg: $"($archive | path basename) must contain only ($binary) at its root" }
    }
    if (use-unzip $archive) {
        run-unzip $archive ['-tqq' $archive] | ignore
    }
}

# Extract the target binary after checking its archive.
export def extract-archive [archive: string, directory: string, binary: string] {
    if (use-unzip $archive) {
        run-unzip $archive ['-q' $archive $binary '-d' $directory] | ignore
    } else {
        run-tar $archive '-xf' '-C' $directory $binary | ignore
    }
}

# The SHA-256 digest of one file as lowercase hexadecimal.
export def file-sha256 [path: string]: nothing -> string {
    open --raw $path | hash sha256
}
