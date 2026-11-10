# Public image library and file command entry.
export use ./image_core.nu [metadata resize]

# Image byte and file operations. Import with `use tola image`.
export def main [] {
  help main
}

# Print metadata for an image file as JSON.
def "main metadata" [source: path] {
  metadata $source | to json
}

# Resize a raster file. With --output, write the file and print its metadata as JSON.
# Without --op, keep the source size, fit one dimension, or fill both dimensions.
def "main resize" [
  source: path
  --output(-o): path
  --width: int
  --height: int
  --op: string
  --anchor: string = "center"
  --format: string = "auto"
  --quality: int
  --filter: string = "lanczos3"
  --background: string
] {
  let options = {
    width: $width, height: $height, op: $op, anchor: $anchor,
    format: $format, quality: $quality, filter: $filter, background: $background
  }
  if $output == null {
    resize $source ...$options
  } else {
    resize $source --output $output ...$options | to json
  }
}
