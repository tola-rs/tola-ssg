# Public image library and file command entry.
export use ./image_core.nu [metadata resize]

# Image byte and file operations. Import with `use tola image`.
export def main [] {
  help main
}

# Print metadata for an image file as JSON.
def "main metadata" [
  source: path # Image file to inspect.
] {
  metadata $source | to json
}

# Resize an image file. With --output, write the file and print its metadata as JSON.
# SVG requires an explicit raster format.
# Without --op, keep the source size, fit one dimension, or fill both dimensions.
def "main resize" [
  source: path # Image file to resize.
  --output(-o): path # New file in an existing directory. Omit to display the encoded bytes in hex.
  --width: int # Output width in pixels.
  --height: int # Output height in pixels.
  --op: string # scale, fit-width, fit-height, fit, or fill.
  --anchor: string = "center" # Which part of the image to keep when cropping.
  --format: string = "auto" # auto, jpeg, jpg, png, or webp. SVG requires a format other than auto.
  --quality: int # JPEG 1–100 or WebP 0–100.
  --filter: string = "lanczos3" # Raster resampling: nearest, triangle, catmull-rom, gaussian, or lanczos3.
  --background: string # Opaque sRGB color, #RRGGBB.
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
