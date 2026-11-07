# Release archives: one executable at the archive root, extraction, and file digests.

# Run tar and fail with its stderr; both archive directions share this one command.
def run-tar [arguments: list<string>] {
    let result = (^tar ...$arguments | complete)
    if $result.exit_code != 0 {
        error make { msg: $"tar failed \(($result.exit_code)\): ($result.stderr | str trim)" }
    }
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
            run-tar [-a -cf $archive -C $stage $binary]
        } else {
            run-tar [-czf $archive -C $stage $binary]
        }
    } finally {
        rm --recursive --force $stage
    }
}

# Extract the target's binary from archive into directory.
export def extract-archive [archive: string, directory: string, target: record] {
    let binary = ($target | get binary)
    if not ($directory | path exists) { mkdir $directory }
    if ($target | get system) == 'windows' {
        run-tar [-xf $archive -C $directory $binary]
    } else {
        run-tar [-xzf $archive -C $directory $binary]
    }
}

# The SHA-256 digest of one file as lowercase hexadecimal.
export def file-sha256 [path: string]: nothing -> string {
    open --raw $path | hash sha256
}
