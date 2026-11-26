# tola-pronunciations

Chinese and Japanese pronunciation lookup tables for slugification.

| Feature | Table | Output | License |
| --- | --- | --- | --- |
| `cedict` | CC-CEDICT | Chinese words (pinyin) | CC BY-SA 4.0 |
| `jmdict` | JMdict | Japanese words (kana) | CC BY-SA 4.0, EDRDG terms |
| `unihan` | Unihan `kMandarin` | Han characters (pinyin) | Unicode License v3 |

No tables are enabled by default. With both `cedict` and `unihan` enabled, CC-CEDICT words take precedence over single-character Unihan entries; `jmdict` never falls back to Mandarin.

```rust
use tola_pronunciations::{chinese_pronunciations, japanese_pronunciations};

assert_eq!(chinese_pronunciations("重庆"), "chong qing");
assert_eq!(japanese_pronunciations("東京"), "とうきょう");
```

Crate code is [MIT](LICENSE). Upstream table versions, licenses, and regeneration scripts are in [`licenses/README.md`](licenses/README.md).
