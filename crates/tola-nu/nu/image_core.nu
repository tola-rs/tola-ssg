use tola-native [inspect-image resize-image]

# Inspect an image from a path or piped bytes.
export def metadata [source?: path]: [nothing -> record, binary -> record] {
  let path = if $source != null { $source | path expand --no-symlink }
  $in | inspect-image $path
}

# Resize an image from a path or piped bytes. SVG requires an explicit raster format.
# Returns binary unless --output is set. File output returns metadata and never overwrites.
# Without --op, keep the source size, fit one dimension, or fill both dimensions.
export def resize [
  source?: path # Source image path, when no bytes are piped in.
  --output(-o): path # New file in an existing directory.
  --width: int # Positive output width.
  --height: int # Positive output height.
  --op: string # scale, fit-width, fit-height, fit, or fill.
  --anchor: string = "center" # center, top, bottom, left, right, or a corner.
  --format: string = "auto" # auto, jpeg, jpg, png, or webp. SVG requires a format other than auto.
  --quality: int # JPEG 1..100; WebP 0..100; PNG ignores quality.
  --filter: string = "lanczos3" # Raster resampling: nearest, triangle, catmull-rom, gaussian, or lanczos3.
  --background: string # Opaque sRGB color, #RRGGBB.
]: [nothing -> any, binary -> any] {
  let source_path = if $source != null { $source | path expand --no-symlink }
  let output_path = if $output != null { $output | path expand --no-symlink }
  $in | resize-image {
    width: $width, height: $height, op: $op, anchor: $anchor,
    format: $format, quality: $quality, filter: $filter, background: $background
  } $source_path $output_path
}
