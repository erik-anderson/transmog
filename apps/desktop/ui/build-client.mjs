import { copyFile, readFile, readdir, writeFile } from 'node:fs/promises';
import { join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as esbuild from 'esbuild';
import { esbuildProjection } from '@microsoft/webui/projection.js';

const workingDirectory = fileURLToPath(new URL('./', import.meta.url));
const outputDirectory = fileURLToPath(new URL('./dist/', import.meta.url));
for (const file of await readdir(join(workingDirectory, 'src'), { recursive: true })) {
  if (!file.endsWith('.html')) continue;
  const template = await readFile(join(workingDirectory, 'src', file), 'utf8');
  if (template.includes('(event)')) {
    throw new Error('WebUI event handlers must pass the reserved event token `e`, not `event`: ' + file);
  }
}

const client = await esbuild.build({
  absWorkingDir: workingDirectory,
  entryPoints: { app: 'src/index.ts', monaco: 'src/monaco.ts' },
  outdir: outputDirectory,
  chunkNames: 'chunks/[name]-[hash]',
  bundle: true,
  splitting: true,
  metafile: true,
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

const outputs = { ...client.metafile.outputs };
for (const [entryPoint, outfile] of [
  ['node_modules/monaco-editor/esm/vs/editor/editor.worker.js', 'monaco-editor.worker.js'],
  ['node_modules/monaco-editor/esm/vs/language/typescript/ts.worker.js', 'monaco-ts.worker.js'],
]) {
  const worker = await esbuild.build({
    absWorkingDir: workingDirectory,
    entryPoints: [join(workingDirectory, entryPoint)],
    outfile: join(outputDirectory, outfile),
    bundle: true,
    format: 'esm',
    minify: true,
    sourcemap: false,
    target: ['es2022'],
    legalComments: 'inline',
    metafile: true,
  });
  Object.assign(outputs, worker.metafile.outputs);
}

// The native origin embeds only the current bundler outputs, including dynamic chunks.
// Split roots each receive an identical CSS output; templates load the Monaco entry once.
const assets = Object.entries(outputs).filter(([file]) => !file.endsWith('.css') || file.endsWith('/monaco.css')).map(([file, metadata]) => {
  const name = relative(outputDirectory, resolve(workingDirectory, file)).replaceAll('\\', '/');
  if (!/^(?:chunks\/)?[a-zA-Z0-9_.-]+\.(?:js|css)$/.test(name)) throw new Error('Unsafe client asset path: ' + name);
  return { path: '/' + name, file: name, contentType: name.endsWith('.css') ? 'text/css; charset=utf-8' : 'text/javascript; charset=utf-8', bytes: metadata.bytes };
}).sort((a, b) => a.path.localeCompare(b.path));
await writeFile(join(outputDirectory, 'client-assets.json'), JSON.stringify(assets, null, 2) + '\n');
await writeFile(join(outputDirectory, 'client-metafile.json'), JSON.stringify(client.metafile, null, 2) + '\n');
await copyFile(join(workingDirectory, 'src/document.css'), join(outputDirectory, 'document.css'));

await copyFile(
  join(workingDirectory, '../icons/icon.svg'),
  join(outputDirectory, 'transmog-icon.svg')
);
