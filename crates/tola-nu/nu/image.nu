# Public image library and file command entry.
export use ./image_core.nu [metadata metadata-file resize resize-file]

# Image byte and file operations. Import with `use tola image`.
export def main [] {
  help main
}

# Print metadata for an image file as JSON.
def "main metadata" [source: path] {
  metadata-file $source | to json
}

# Resize a raster file to an explicit destination and print its metadata as JSON.
def "main resize" [
  source: path
  destination: path
  --width: int
  --height: int
  --op: string = "fill"
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
  resize-file $source $destination ...$options | to json
}
