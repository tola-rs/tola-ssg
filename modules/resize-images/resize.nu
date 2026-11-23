use tola [get-assets get-context get-hook get-module image]

const source_types = ['image/jpeg' 'image/png' 'image/webp']

def resize-options [
    options: record<widths: list<int>, format: string, upscale: bool>
    key: string
]: nothing -> record {
    let widths = $options.widths
    let valid_widths = ($widths | all {|width| $width > 0 and $width <= 0xffffffff })
    if ($widths | is-empty) or not $valid_widths {
        error make {msg: $"`($key).widths` must list positive pixel widths"}
    }
    let format = $options.format
    if $format not-in ['jpg' 'png' 'webp'] {
        error make {msg: $"`($key).format` must be jpg, png, or webp"}
    }
    let quality = $options.quality?
    if $quality != null {
        let minimum = if $format == 'jpg' { 1 } else { 0 }
        if ($quality | describe) != 'int' or $quality < $minimum or $quality > 100 {
            error make {msg: $"`($key).quality` must be an integer from ($minimum) to 100"}
        }
    }
    {widths: ($widths | uniq | sort), format: $format, quality: $quality, upscale: $options.upscale}
}

def image-options [defaults: record, overrides: record, source: string, key: string]: nothing -> record {
    let override = ($overrides | get -o $source)
    if $override == null { return $defaults }

    let key = $"($key).overrides.\"($source)\""
    let fields = ($defaults | columns)
    let unknown = ($override | columns | where {|field| $field not-in $fields })
    if not ($unknown | is-empty) {
        error make {msg: $"`($key)` has unknown settings ($unknown | str join ', ')"}
    }
    resize-options ($defaults | merge $override) $key
}

def select-images [options: record]: nothing -> list<record> {
    let include = $options.include
    let exclude = ($options.exclude? | default [])
    let source = $options.source?
    let assets = if $source == null {
        get-assets --include $include --exclude $exclude
    } else {
        get-assets --include $include --exclude $exclude --module $source.module
    }
    $assets
    | where {|asset| $asset.media-type in $source_types }
    | sort-by source output
    | uniq-by source
}

def read-variant [file: string] {
    try {
        let stored = (open --raw $file | into binary)
        let digest = ($stored | bytes at 0..<32)
        let bytes = ($stored | bytes at 32..)
        if ($bytes | hash sha256 --binary) == $digest { $bytes }
    }
}

def store-variant [file: string, bytes: binary] {
    let temporary = $"($file).(random uuid).tmp"
    try {
        [($bytes | hash sha256 --binary) $bytes] | bytes collect | save --raw $temporary
        mv --force $temporary $file
    } catch {
        try { rm --force $temporary }
    }
}

def resize-asset [asset: record, options: record, dirs: record, encoder: string] {
    let source = ($dirs.input | path join $asset.output)
    let original = (open --raw $source | into binary)
    let size = ($original | image metadata)
    if $size.format == 'svg' { error make {msg: 'expected a raster image'} }
    let widths = ($options.widths | where {|width| $options.upscale or $width <= $size.width })
    if ($widths | is-empty) { return }

    let folder = ($dirs.output | path join $asset.source)
    let digest = ($original | hash sha256)
    let format = $options.format
    let quality = $options.quality
    mkdir $folder
    for width in $widths {
        let key = ([$encoder $digest $width $format $quality] | to nuon | hash sha256)
        let cache = ($dirs.cache | path join $key)
        let bytes = match (read-variant $cache) {
            null => {
                let rendered = ($original | image resize --width $width --format $format --quality $quality | into binary)
                store-variant $cache $rendered
                $rendered
            }
            $cached => $cached
        }
        $bytes | save --raw ($folder | path join $"w($width).($format)")
    }
}

let module = get-module
let hook = get-hook
let options = $module.options
let key = $"modules.($module.name).options"
let jobs = $options.jobs
if ($jobs | describe) != 'int' or $jobs < 1 {
    error make {msg: $"`($key).jobs` must be a positive integer"}
}
let defaults = (resize-options $options $key)
let overrides = ($options.overrides? | default {})
let dirs = {
    input: $hook.input-dir
    output: ($hook.output-dir | path join $hook.outputs.0.tree)
    cache: $hook.cache-dir
}

# Create the declared tree even when no image qualifies.
mkdir $dirs.output
let assets = (select-images $options)
if ($assets | is-empty) { return }

let executable = (get-context | get executable)
let encoder = (open --raw $executable | hash sha256)
let threads = ([$jobs ($assets | length)] | math min)
$assets | par-each --threads $threads --keep-order {|asset|
    let settings = (image-options $defaults $overrides $asset.source $key)
    try {
        resize-asset $asset $settings $dirs $encoder
    } catch {|error|
        error make {msg: $"could not resize `($asset.source)`: ($error.msg)"}
    }
} | ignore
