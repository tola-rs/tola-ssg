import { expect } from '@std/expect'
import { describe, test } from '@std/testing/bdd'

import { jmdict, sorted } from './index.ts'

describe('pronunciation tables', () => {
  test('a table is ordered by the bytes the lookup compares', () => {
    // U+FA19 sorts before U+20BB7 by code point and after it as UTF-16 units.
    const entries = sorted(
      [
        ['𠮷', 'a'],
        ['神', 'b'],
        ['z', 'c'],
        ['a', 'd'],
      ] as const,
    )
    expect(entries.map(([name]) => name)).toEqual(['a', 'z', '神', '𠮷'])
  })

  test('restrictions select applicable readings', () => {
    // EDRDG entry 1156010: https://www.edrdg.org/jmdict_edict_list/2007/msg00657.html
    const table = jmdict(`<!-- JMdict created: 2026-09-26 -->
<JMdict>
<entry>
<ent_seq>1156010</ent_seq>
<k_ele><keb>囲繞</keb></k_ele>
<k_ele><keb>囲にょう</keb></k_ele>
<r_ele><reb>いじょう</reb><re_restr>囲繞</re_restr></r_ele>
<r_ele><reb>いにょう</reb></r_ele>
</entry>
</JMdict>`)
    expect(table.entries).toEqual([
      ['囲にょう', 'いにょう'],
      ['囲繞', 'いじょう'],
    ])
  })

  test('restrictions include every named spelling', () => {
    const table = jmdict(`<!-- JMdict created: 2026-09-26 -->
<JMdict>
<entry>
<k_ele><keb>取扱い</keb></k_ele>
<k_ele><keb>取り扱い</keb></k_ele>
<k_ele><keb>取扱</keb></k_ele>
<r_ele>
<reb>とりあつかい</reb>
<re_restr>取扱い</re_restr>
<re_restr>取り扱い</re_restr>
</r_ele>
</entry>
</JMdict>`)
    expect(table.entries).toEqual([
      ['取り扱い', 'とりあつかい'],
      ['取扱い', 'とりあつかい'],
    ])
  })

  test('kana-only readings cannot name kanji', () => {
    const table = jmdict(`<!-- JMdict created: 2026-09-26 -->
<JMdict>
<entry>
<k_ele><keb>珈琲</keb></k_ele>
<r_ele><reb>コーヒー</reb><re_nokanji/></r_ele>
<r_ele><reb>こーひー</reb></r_ele>
</entry>
<entry>
<k_ele><keb>亜米利加</keb></k_ele>
<r_ele><reb>アメリカ</reb><re_nokanji/></r_ele>
</entry>
</JMdict>`)
    expect(table.entries).toEqual([['珈琲', 'こーひー']])
  })

  test('unsupported readings leave later choices', () => {
    const table = jmdict(`<!-- JMdict created: 2026-09-26 -->
<JMdict>
<entry>
<k_ele><keb>一番</keb></k_ele>
<r_ele><reb>1ばん</reb></r_ele>
<r_ele><reb>いちばん</reb></r_ele>
<r_ele><reb>ひとつがい</reb></r_ele>
</entry>
</JMdict>`)
    expect(table.entries).toEqual([['一番', 'いちばん']])
  })

  test('unrestricted readings cover Han spellings', () => {
    const table = jmdict(`<!-- JMdict created: 2026-09-26 -->
<JMdict>
<entry>
<k_ele><keb>うｐ主</keb></k_ele>
<k_ele><keb>うp主</keb></k_ele>
<k_ele><keb>うぷぬし</keb></k_ele>
<r_ele><reb>うぷぬし</reb></r_ele>
</entry>
</JMdict>`)
    expect(table.entries).toEqual([
      ['うp主', 'うぷぬし'],
      ['うｐ主', 'うぷぬし'],
    ])
  })

  test('duplicate spellings keep the first entry', () => {
    const table = jmdict(`<!-- JMdict created: 2026-09-26 -->
<JMdict>
<entry>
<k_ele><keb>生物</keb></k_ele>
<r_ele><reb>せいぶつ</reb></r_ele>
</entry>
<entry>
<k_ele><keb>生物</keb></k_ele>
<r_ele><reb>なまもの</reb></r_ele>
</entry>
</JMdict>`)
    expect(table.entries).toEqual([['生物', 'せいぶつ']])
  })

  test('a spelling without Han cannot name a pronunciation', () => {
    // tola-pronunciations returns ASCII text without consulting a table, so a key that is
    // entirely ASCII would be looked up by no caller and silently never match.
    const table = jmdict(`<!-- JMdict created: 2026-09-26 -->
<JMdict>
<entry>
<k_ele><keb>AB</keb></k_ele>
<k_ele><keb>漢字</keb></k_ele>
<r_ele><reb>かんじ</reb></r_ele>
</entry>
</JMdict>`)
    expect(table.entries).toEqual([['漢字', 'かんじ']])
  })
})
