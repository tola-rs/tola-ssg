import { readFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { WASI } from 'node:wasi';

const [modulePath, outputDirectory, ...extra] = process.argv.slice(2);
if (!modulePath || !outputDirectory || extra.length) {
  throw new Error('usage: node run-wasi.mjs CORPUS.wasm OUTPUT_DIRECTORY');
}
await mkdir(outputDirectory, { recursive: true });
const wasi = new WASI({
  version: 'preview1',
  args: [modulePath, '/output'],
  env: {},
  preopens: { '/output': resolve(outputDirectory) },
});
const module = await WebAssembly.compile(await readFile(modulePath));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
wasi.start(instance);
