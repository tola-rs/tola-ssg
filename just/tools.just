set working-directory := '..'

# Parse every Nushell script without running it.
[default]
check:
    for file in tools/*.nu tools/*/*.nu; do nu -c "use $file *" || exit 1; done

# Regenerate the Iconify collection index Tola pins; name collections to refresh only those.
icons-index *collections:
    nu tools/assets/icons.nu {{ collections }}

# Regenerate a pronunciation table from the source file it is derived from.
pronunciations *args:
    nu tools/assets/pronunciations.nu {{ args }}

# Regenerate the fonts from the pinned typst-assets release; pass --check to verify without writing.
fonts *args:
    nu tools/assets/fonts.nu {{ args }}
