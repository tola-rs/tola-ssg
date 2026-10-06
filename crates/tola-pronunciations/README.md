# tola-pronunciations

The pronunciations of text: the tables this repository ships, and the lookup over them.
`chinese_pronunciations` reads Han text and prefers the longest name it knows, so `重庆` comes back
`chong qing` rather than the readings of `重` and `庆`; text no table names keeps its own
characters. `japanese_pronunciations` reads only JMdict and returns kana (`東京` → `とうきょう`).

Each table is one Cargo feature:

| Feature | Table | Pronunciations | Terms |
| --- | --- | --- | --- |
| `cedict` | CC-CEDICT | Chinese words | CC BY-SA 4.0 |
| `jmdict` | JMdict | Japanese words, in kana | CC BY-SA 4.0, EDRDG terms |
| `unihan` | Unihan `kMandarin` | Han characters | Unicode License v3 |

With both Chinese tables enabled, CC-CEDICT words win over Unihan characters; `jmdict` never falls
back to Mandarin.

## Usage

Nothing is enabled by default. Enable `unihan` for the character lookup:

```sh
cargo add tola-pronunciations --features unihan
```

```rust
use tola_pronunciations::chinese_pronunciations;

assert_eq!(chinese_pronunciations("中"), "zhong");
assert_eq!(chinese_pronunciations("tola"), "tola");
```

Enable `jmdict` for Japanese words, in kana:

```rust
use tola_pronunciations::japanese_pronunciations;

assert_eq!(japanese_pronunciations("東京"), "とうきょう");
```

Features pick tables, not languages or regions.

## Sources and licence

The code is [MIT](LICENSE). The tables keep their upstream licences;
[licenses/README.md](licenses/README.md) records each one's source, version, extraction rules, and
regeneration commands, next to the unmodified upstream files.
