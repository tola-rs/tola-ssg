# Inspect image bytes using the same decoder as the Tola build engine.
export def metadata []: binary -> record {
  $in | __tola_image_metadata
}

# Inspect a file relative to the current Nu PWD.
export def metadata-file [source: path]: nothing -> record {
  __tola_image_metadata_file ($source | path expand --no-symlink)
}

# Resize raster bytes. SVG rendering requires the build engine's font/resource context.
# Defaults: centered fill, automatic format, Lanczos3; quality follows the shared codec.
export def resize [
  --width: int # Positive output width.
  --height: int # Positive output height.
  --op: string = "fill" # scale, fit-width, fit-height, fit, or fill.
  --anchor: string = "center" # center, top, bottom, left, right, or a corner.
  --format: string = "auto" # auto, jpeg, jpg, png, or webp.
  --quality: int # JPEG 1..100; WebP 0..100; PNG ignores quality.
  --filter: string = "lanczos3" # nearest, triangle, catmull-rom, gaussian, or lanczos3.
  --background: string # Opaque sRGB color, #RRGGBB.
]: binary -> binary {
  $in | __tola_image_resize {
    width: $width, height: $height, op: $op, anchor: $anchor,
    format: $format, quality: $quality, filter: $filter, background: $background
  }
}

# Resize a raster file into a new destination; existing files are never overwritten.
# Only metadata is returned. The destination directory must already exist.
export def resize-file [
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
]: nothing -> record {
  __tola_image_resize_file ($source | path expand --no-symlink) ($destination | path expand --no-symlink) {
    width: $width, height: $height, op: $op, anchor: $anchor,
    format: $format, quality: $quality, filter: $filter, background: $background
  }
}
