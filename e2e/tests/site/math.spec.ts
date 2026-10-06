import { expect } from '@playwright/test'
import { writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { ONE_PIXEL_PNG } from '../../support/media.ts'
import { test } from '../../support/server.ts'

const MATH_PAGE = String.raw`#import "@tola/web:0.0.0": math-svg
#html.style(".baseline { display: inline-block; width: 0; height: 0; } .sample { margin: 1em; } .sample, .pair { white-space: nowrap; }")
#for size in (9pt, 18pt, 36pt) {
  for leading in (0em, 0.65em, 3em) {
    for bounds in (false, true) {
      set text(size: size,
        top-edge: if bounds { "bounds" } else { "cap-height" },
        bottom-edge: if bounds { "bounds" } else { "baseline" },
      )
      set par(leading: leading)
      for equation in ($x$, $x + y_2$, $x + 1 / y_2$, $x + 1 / (1 + 1 / y_2)$,
        $x + sqrt(1 / y_2)$, $x + mat(1, 2; 3, 4)$) {
        html.p(class: "sample math")[
          #html.span(class: "baseline")[]#math-svg(equation)
        ]
      }
    }
  }
}
#for shift in (-0.5, 0, 0.5, 2) {
  html.elem("p", attrs: (class: "sample box", data-shift: json.encode(shift)))[
    #html.span(class: "baseline")[]#box(html.frame(
      box(baseline: shift * 1em, rect(width: 1em, height: 1em, fill: red, stroke: none)),
    ))
  ]
}
#for body in (
  pad(top: 0.4em, bottom: 0.7em, $x + 1 / y_2$),
  pad(bottom: -0.3em, $x$),
  scale(x: 150%, y: 80%, reflow: true, $x + 1 / y_2$),
) {
  html.p(class: "sample math")[
    #html.span(class: "baseline")[]#box(html.frame(body))
  ]
}
#html.elem("p", attrs: (class: "sample math", data-shift: "0.5"))[
  #html.span(class: "baseline")[]#box(html.frame(move(dy: 0.5em, $x$)))
]
// Inputs adapted to inline export from typst/typst#8729, #5426 and #6477.
#let scr(it) = text(features: ("ss01",), box($cal(it)$))
#show math.equation: math-svg
#for equation in (
  $x + 1 / (2 / (3 / (4 / 5)))$,
  $S = overbrace(beta (alpha) S I, "one line") - overbrace(mu (N), "two" \ "line")$,
  $S = underbrace(beta (alpha) S I, "one line") - underbrace(mu (N), "two" \ "line")$,
  $A scr(A)$,
) {
  html.p(class: "sample math")[
    #html.span(class: "baseline")[]#equation
  ]
}
// Non-math inputs: https://forum.typst.app/t/whats-going-on-in-these-box/7857
#for body in (
  [#set text(bottom-edge: "descender"); #box[g];a],
  [#box(width: 0pt)[1.(abc)]#h(1em)],
  [gyp \ second line],
  block(width: 12em)[#lorem(1000)],
) {
  html.p(class: "sample math")[
    #html.span(class: "baseline")[]#box(html.frame(body))
  ]
}
// Negative inset example: https://forum.typst.app/t/7894
#for inset in (0em, -0.5em) {
  html.p(class: "sample image")[
    #html.span(class: "baseline")[]#box(html.frame(
      box(height: 1em + inset, inset: (top: inset), baseline: 0%, image("icon.png", height: 1em)),
    ))
  ]
}
#math-svg($ x^2 $)
#html.p(id: "flow")[
  #for _ in range(100) {
    [A long paragraph #html.span(class: "pair")[#html.span(class: "baseline")[]#math-svg($x + 1 / y_2$)]. ]
  }
]
`

test('inline frames follow text baselines', async ({ page, sites }) => {
  const site = await sites.preview({
    initialContent: { relativePath: 'index.typ', source: MATH_PAGE },
    beforeStart: (root) => writeFile(join(root, 'content/icon.png'), ONE_PIXEL_PNG),
  })
  await page.goto(site.url, { waitUntil: 'load' })
  for (const fontSize of [12, 24, 48]) {
    await page.locator('body').evaluate((body, size) => {
      body.style.fontSize = `${size}px`
    }, fontSize)
    const offsets = await page.locator('.sample').evaluateAll((rows) =>
      rows.map((row, index) => {
        const svg = row.querySelector('svg')!
        const shift = Number(row.getAttribute('data-shift')) * parseFloat(getComputedStyle(svg).fontSize)
        const baseline = row.querySelector('.baseline')!.getBoundingClientRect().bottom + shift
        if (row.classList.contains('math')) {
          // Each text frame starts on its first glyph's baseline, including multiline frames.
          const glyph = svg.querySelector('use')!
          return { index, offset: glyph.getScreenCTM()!.f - baseline }
        }
        const shape = svg.querySelector(row.classList.contains('image') ? 'image' : 'path')!
        return { index, offset: shape.getBoundingClientRect().bottom - baseline }
      })
    )
    expect(offsets).toHaveLength(126)
    // Allow Chromium's subpixel layout and Typst's rounding of SVG lengths.
    for (const { index, offset } of offsets) {
      expect(Math.abs(offset), `frame ${index}, font size ${fontSize}`).toBeLessThan(0.08)
    }
  }
  await expect(page.locator('.tola-math-block svg')).toHaveCSS('display', 'block')
  expect(await page.locator('.tola-math-block svg').evaluate((svg) => svg.style.verticalAlign)).toBe('')
  for (const width of [320, 960]) {
    await page.setViewportSize({ width, height: 720 })
    const lines = await page.locator('#flow .pair').evaluateAll((pairs) =>
      pairs.map((pair) => {
        const baseline = pair.querySelector('.baseline')!.getBoundingClientRect().bottom
        return { baseline, offset: pair.querySelector('use')!.getScreenCTM()!.f - baseline }
      })
    )
    expect(lines).toHaveLength(100)
    expect(new Set(lines.map((line) => line.baseline)).size).toBeGreaterThan(1)
    for (const line of lines) expect(Math.abs(line.offset)).toBeLessThan(0.08)
  }
})
