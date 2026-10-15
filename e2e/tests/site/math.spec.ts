import { expect } from '@playwright/test'
import { test } from '../../support/server.ts'

const MATH_PAGE = String
  .raw`#html.p(id: "inline")[#html.span(class: "before")[Before ]$x + y_2$#html.span(class: "after")[ after]]
$ x^2 + sqrt(y) $
`

test('inline and block math render as semantic MathML', async ({ page, sites }) => {
  const site = await sites.preview({
    initialContent: { relativePath: 'index.typ', source: MATH_PAGE },
  })
  await page.goto(site.url, { waitUntil: 'load' })

  const inline = page.locator('#inline math')
  const block = page.locator('math[display="block"]')
  await expect(inline.locator('msub')).toHaveCount(1)
  await expect(block.locator('msup')).toHaveCount(1)
  await expect(block.locator('msqrt')).toHaveCount(1)
  await expect(block).toBeVisible()

  for (const width of [320, 960]) {
    await page.setViewportSize({ width, height: 720 })
    const positions = await page.locator('#inline').evaluate((row) => {
      const before = row.querySelector('.before')!.getBoundingClientRect()
      const after = row.querySelector('.after')!.getBoundingClientRect()
      const math = row.querySelector('math')!.getBoundingClientRect()
      return {
        before: before.top,
        after: after.top,
        math: math.top,
        bottom: math.bottom,
        line: before.bottom,
      }
    })
    expect(Math.abs(positions.before - positions.after)).toBeLessThan(1)
    expect(positions.math).toBeLessThan(positions.line)
    expect(positions.bottom).toBeGreaterThan(positions.before)
  }
})

test('explicit SVG math keeps inline and block layout', async ({ page, sites }) => {
  const site = await sites.preview({
    initialContent: {
      relativePath: 'index.typ',
      source: String.raw`#let math-svg(equation) = if equation.at("block", default: false) {
  block(html.frame(equation))
} else {
  box(html.frame(equation))
}
#show math.equation: math-svg
#html.p(id: "inline")[Before $x$ after]
$ x^2 $
`,
    },
  })
  await page.goto(site.url, { waitUntil: 'load' })
  await expect(page.locator('math')).toHaveCount(0)
  const frames = page.locator('svg')
  await expect(frames).toHaveCount(2)
  await expect(frames.first()).toHaveCSS('display', 'inline')
  await expect(frames.last()).toHaveCSS('display', 'block')
})
