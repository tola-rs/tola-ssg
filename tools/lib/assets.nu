# Primitives the asset generators share: fetching a release and writing a zstd frame.

# Fetch one URL into one file.
export def download [url: string, path: string] {
    let fetched = (^curl --fail --location --silent --show-error --output $path $url | complete)
    if $fetched.exit_code != 0 { error make { msg: $"($url) is unavailable" } }
}

# Compress one file into one zstd frame: level 19, no content size, no checksum.
export def compress-zstd [input: string, output: string] {
    let compressed = (^zstd -19 --no-content-size --no-check -q -f -o $output $input | complete)
    if $compressed.exit_code != 0 {
        error make { msg: $"zstd refused to write the frame for ($input | path basename)" }
    }
}
