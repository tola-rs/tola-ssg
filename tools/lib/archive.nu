# Release archives: one executable at the archive root, extraction, and file digests.

# ZIP needs bsdtar. Windows ships it, but Git Bash can put GNU tar first on PATH.
def run-tar [archive: string, flags: string, ...operands: string]: nothing -> string {
    let tar = if $nu.os-info.name == 'windows' {
        $env.SystemRoot | path join 'System32' 'tar.exe'
    } else if ($archive | str ends-with '.zip') {
        'bsdtar'
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
    run-tar $archive '-tf' | lines
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
            # bsdtar chooses the zip format from the archive extension.
            run-tar $archive '-acf' '-C' $stage $binary | ignore
        } else {
            create-tar-gz $archive $stage [$binary]
        }
    } finally {
        rm --recursive --force $stage
    }
}

# Extract the target's binary from archive into directory.
export def extract-archive [archive: string, directory: string, target: record] {
    let binary = ($target | get binary)
    if (archive-members $archive) != [$binary] {
        error make { msg: $"($archive | path basename) must contain only ($binary) at its root" }
    }
    if not ($directory | path exists) { mkdir $directory }
    run-tar $archive '-xf' '-C' $directory $binary | ignore
}

# The SHA-256 digest of one file as lowercase hexadecimal.
export def file-sha256 [path: string]: nothing -> string {
    open --raw $path | hash sha256
}
