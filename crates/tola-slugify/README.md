# tola-slugify

Turns text into a slug, and nothing else. `slugify_segment` takes a segment's text plus a
`NamingRules` value (characters, case, separator, pronunciation) and returns the slug that text
spells, or `None` if it spells none.

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

Only allowed characters survive: letters, marks, numbers, and `- . _ ~ ! $ & ' + , ; = @`.
Everything else becomes the separator. Input and output are NFC normalized.

In `ascii` mode, Han text is romanized by the language in the rules: `ja` reads JMdict
(`東京` → `とうきょう` → `toukyou`), anything else reads CC-CEDICT and falls back to Unihan. Kana
use Hepburn, Hangul uses Revised Romanization; the tables are the `han-tables` feature, on by
default.
