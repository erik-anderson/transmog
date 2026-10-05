import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';
import * as esbuild from 'esbuild';
import { esbuildProjection } from '@microsoft/webui/projection.js';

const check = process.argv.includes('--check');
const workingDirectory = fileURLToPath(new URL('./', import.meta.url));
const outputDirectory = fileURLToPath(new URL('./dist/', import.meta.url));
const expected = check
  ? new Map(await Promise.all(
      ['app.js', 'webui-projection.json'].map(async (filename) => [
        filename,
        await readFile(join(outputDirectory, filename))
      ])
    ))
  : undefined;

await esbuild.build({
  absWorkingDir: workingDirectory,
  entryPoints: [join(workingDirectory, 'src/index.ts')],
  outfile: join(outputDirectory, 'app.js'),
  bundle: true,
  format: 'esm',
  minify: true,
  sourcemap: false,
  target: ['es2022'],
  define: {
    __WEBUI_DEV__: 'false'
  },
  legalComments: 'inline',
  plugins: [esbuildProjection()]
});

if (expected !== undefined) {
  for (const [filename, previous] of expected) {
    const generated = await readFile(join(outputDirectory, filename));
    if (!previous.equals(generated)) {
      throw new Error(`${filename} is stale; commit the regenerated file`);
    }
  }
}
