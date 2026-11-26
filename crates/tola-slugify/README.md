# tola-slugify

Turns text into NFC-normalized URL slugs with optional ASCII romanization for Han, kana, and Hangul.

```rust
use tola_slugify::{
    HanPronunciations, NamingRules, SlugCase, SlugMode, SlugSeparator, slugify_segment,
};

let ascii = NamingRules {
    mode: SlugMode::Ascii,
    case: SlugCase::Lower,
    separator: SlugSeparator::Dash,
    pronunciations: HanPronunciations::Chinese,
};
assert_eq!(slugify_segment("北京 Café", ascii).as_deref(), Some("bei-jing-cafe"));
```

Returns `None` when no allowed characters remain. In `Ascii` mode, Japanese uses JMdict + Hepburn, Chinese uses CC-CEDICT + Unihan (`han-tables` feature, on by default), and Korean uses Revised Romanization.
