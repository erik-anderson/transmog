import {
  brotliCompressSync,
  deflateSync,
  gzipSync,
  zstdCompressSync,
} from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const output = process.argv[2];
if (!output) throw new Error('usage: node generate-encoded-fixtures.mjs OUTPUT_DIRECTORY');
if (typeof zstdCompressSync !== 'function') {
  throw new Error('Node.js 24 or newer with built-in zstd support is required');
}

mkdirSync(output, { recursive: true });
const encoders = {
  gzip: gzipSync,
  br: brotliCompressSync,
  deflate: deflateSync,
  zstd: zstdCompressSync,
};

for (const [name, encode] of Object.entries(encoders)) {
  writeFileSync(path.join(output, name), encode(body(name)));
}

let stacked = body('gzip-br-deflate-zstd');
for (const encode of [gzipSync, brotliCompressSync, deflateSync, zstdCompressSync]) {
  stacked = encode(stacked);
}
writeFileSync(path.join(output, 'stacked'), stacked);

function body(coding) {
  return Buffer.from(
    `<!doctype html><html lang="en"><head><meta charset="utf-8">`
      + `<title>${coding} interop fixture</title></head>`
      + `<body><h1 data-origin="nginx" data-coding="${coding}">`
      + `${coding} through Transmog</h1></body></html>`,
    'utf8',
  );
}
