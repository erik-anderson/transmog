import { copyFile, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as esbuild from 'esbuild';
import { esbuildProjection } from '@microsoft/webui/projection.js';

const workingDirectory = fileURLToPath(new URL('./', import.meta.url));
const outputDirectory = fileURLToPath(new URL('./dist/', import.meta.url));
const shellTemplate = await readFile(
  join(workingDirectory, 'src/transmog-app-shell/transmog-app-shell.html'),
  'utf8'
);
if (shellTemplate.includes('(event)')) {
  throw new Error('WebUI event handlers must pass the reserved event token `e`, not `event`');
}

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
  loader: { '.ttf': 'dataurl' },
  plugins: [esbuildProjection()]
});

for (const [entryPoint, outfile] of [
  ['node_modules/monaco-editor/esm/vs/editor/editor.worker.js', 'monaco-editor.worker.js'],
  ['node_modules/monaco-editor/esm/vs/language/typescript/ts.worker.js', 'monaco-ts.worker.js'],
]) {
  await esbuild.build({
    absWorkingDir: workingDirectory,
    entryPoints: [join(workingDirectory, entryPoint)],
    outfile: join(outputDirectory, outfile),
    bundle: true,
    format: 'esm',
    minify: true,
    sourcemap: false,
    target: ['es2022'],
    legalComments: 'inline',
  });
}

await copyFile(
  join(workingDirectory, '../icons/icon.svg'),
  join(outputDirectory, 'transmog-icon.svg')
);
