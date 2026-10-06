import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

/** Every content source renders, so an unsaved edit to any of them reaches the compiler. */
export const RENDER_EVERY_SOURCE = `#import "@tola/source:0.0.0": all-sources
#import "@tola/address:0.0.0": decode-url-path, route, route-to-output, slugify
#for source in all-sources() {
  let declared = if source.meta == none { none } else {
    source.meta.at("permalink", default: none)
  }
  let route = if declared == none {
    route(source.route-segments.map(segment => slugify(segment)))
  } else {
    decode-url-path(declared)
  }
  document(route-to-output(route), format: "html")[#include source.file]
}
`

/** Creates the content root empty; a caller that needs a content source writes one. */
export async function writeMinimalSite(
  root: string,
  { program = '#document("index.html")[Home]\n' }: { program?: string } = {},
): Promise<void> {
  await mkdir(join(root, 'content'), { recursive: true })
  await writeFile(join(root, 'tola.toml'), '')
  await writeFile(join(root, 'site.typ'), program)
}

/** Writes the minimal site whose program renders every content source. */
export async function writeRenderingSite(root: string): Promise<void> {
  await writeMinimalSite(root, { program: RENDER_EVERY_SOURCE })
}

/**
 * The path and file URI of a document in the site's content root, `document.typ` by default; a
 * caller that needs the file on disk writes it.
 */
export function contentDocument(root: string, name = 'document.typ'): { path: string; uri: string } {
  const path = join(root, 'content', name)
  return { path, uri: pathToFileURL(path).href }
}

export async function writeSiteProgram(root: string, program: string): Promise<void> {
  await writeFile(join(root, 'site.typ'), program)
}
