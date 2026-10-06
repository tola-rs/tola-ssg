/**
 * @module
 * Where this package and the checkout it maintains are. Entry points resolve their inputs from these
 * instead of deriving the same path from their own location, and `import.meta.dirname` stays unread:
 * the type is optional because a module can come from a non-file URL, while every module here is a
 * file.
 */

import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/** The maintenance package itself. */
export const PACKAGE_ROOT = dirname(fileURLToPath(import.meta.url))

/** The Tola checkout this package maintains. */
export const REPOSITORY_ROOT = resolve(PACKAGE_ROOT, '..')
