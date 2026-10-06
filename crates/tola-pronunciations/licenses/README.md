# Pronunciation sources

## CC-CEDICT — [`data/cedict.zst`](../data/cedict.zst)

Source: [MDBG CC-CEDICT](https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz),
2026-09-26 (125115 source entries).
Copyright (C) 1997, 1998 Paul Andrew Denisowski and the CC-CEDICT contributors.
Licence: [CC BY-SA 4.0](LICENSE-CC-BY-SA-4.0).

Both spellings are retained when they contain at least two Unicode scalar values
and at least one Han character. Tone numbers are removed; `u:` and `v` (spellings
of `ü`) become `u`. Unsupported pronunciations are omitted. The first retained
entry for a word wins.

## JMdict — [`data/jmdict.zst`](../data/jmdict.zst)

Source: [JMdict](https://www.edrdg.org/pub/Nihongo/JMdict_e.gz), created 2026-09-26.
Copyright © the Electronic Dictionary Research and Development Group (EDRDG).
Licence: [CC BY-SA 4.0](LICENSE-CC-BY-SA-4.0) and the [EDRDG terms](EDRDG-LICENCE.html).

For each spelling containing Han, select the first supported kana reading that
applies under `re_restr`, excluding readings marked `re_nokanji`. Spellings with
no such reading are omitted; otherwise the upstream spelling is kept unchanged.
The first retained entry for a spelling wins.

## Unihan — [`data/unihan.zst`](../data/unihan.zst)

Source: [Unicode Unihan](https://www.unicode.org/Public/UCD/latest/ucd/Unihan.zip),
Unicode 18.0.0, source date 2026-07-31 00:00:00 GMT [KL].
Copyright © 1991-2026 Unicode, Inc.
Licence: [Unicode License v3](LICENSE-UNICODE), including its full copyright and
permission notice.

Only `kMandarin` is retained, taking the first reading of each value. Tone marks
and the diaeresis in `ü` are removed, so `ü` becomes `u`; output is lowercased.

All tables are sorted by spelling. Selection does not use frequency tags,
surrounding text, or regional preferences.

## Unmodified upstream files

| Local copy | Upstream |
|---|---|
| [LICENSE-UNICODE](LICENSE-UNICODE) | <https://www.unicode.org/license.txt> |
| [LICENSE-CC-BY-SA-4.0](LICENSE-CC-BY-SA-4.0) | <https://creativecommons.org/licenses/by-sa/4.0/legalcode.txt> |
| [EDRDG-LICENCE.html](EDRDG-LICENCE.html) | <https://www.edrdg.org/edrdg/licence.html> |
| [JMDICT-DOCUMENTATION.html](JMDICT-DOCUMENTATION.html) | <https://www.edrdg.org/wiki/JMdict-EDICT_Dictionary_Project.html> |
| [JMDICT-DTD.xml](JMDICT-DTD.xml) | <https://www.edrdg.org/jmdict/dtd-jmdict.xml> |

## Regeneration

From the repository root, pass the official source files to the generator.
Extract `Unihan_Readings.txt` from `Unihan.zip` first:

```sh
just scripts::pronunciations unihan /path/to/Unihan_Readings.txt
just scripts::pronunciations cedict /path/to/cedict_1_0_ts_utf-8_mdbg.txt.gz
just scripts::pronunciations jmdict /path/to/JMdict_e.gz
```

Reuse the recorded source snapshot to reproduce a table. For regular source
updates, download current official files, regenerate, review the diff and source
dates, and update the snapshot facts here. Replace changed upstream licence and
documentation copies verbatim. EDRDG's section 4 describes its update requirement.

Fix extraction in `scripts/pronunciations/index.ts` and dictionary errors upstream,
then regenerate; do not edit generated entries or maintain a local correction
table. The generator compresses each table into a zstd frame; builds decode the
committed frames without downloading or regenerating them.
