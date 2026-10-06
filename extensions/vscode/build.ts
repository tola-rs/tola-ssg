import * as esbuild from 'esbuild'
import * as fs from 'node:fs/promises'

// `--production` is the VSIX build the `package` task runs; it omits source maps so the shipped
// bundle never references a file the package omits.
const production = process.argv.includes('--production')

async function build(): Promise<void> {
  // esbuild's JS API has no `clean` option and leaves earlier outputs in place.
  await fs.rm('dist', { recursive: true, force: true })
  await esbuild.build({
    entryPoints: { extension: 'src/extension.ts', test: 'src/test/suite.ts' },
    outdir: 'dist',
    outExtension: { '.js': '.cjs' },
    bundle: true,
    platform: 'node',
    format: 'cjs',
    target: 'node20',
    external: ['vscode'],
    sourcemap: !production,
    minify: production,
  })
}

await build().catch((error: unknown) => {
  console.error(error)
  process.exitCode = 1
})
