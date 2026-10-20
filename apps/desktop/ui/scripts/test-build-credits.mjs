import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm, symlink } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { cargoCredits,extensionCredits } from './build-credits.mjs';

const repository=fileURLToPath(new URL('../../../../',import.meta.url));
const sources=JSON.parse(await readFile(resolve(repository,'third_party/rust-transport/sources.json'),'utf8'));
const packages=Object.entries(sources).map(([name,source])=>({
  id:name,name,version:source.version,source:null,license:name==='quiche'?'BSD-2-Clause':'MIT',
  manifest_path:resolve(repository,'third_party/rust-transport/vendor',name,'Cargo.toml'),
}));

test('credits retain the pinned MAPI reference and its complete license',async()=>{
  const entries=await extensionCredits(repository);
  assert.equal(entries.length,1);
  assert.equal(entries[0].version,'7a76470574377d14cae612c507f45b34d70e05a0');
  assert.equal(entries[0].text,await readFile(resolve(repository,'third_party/mapi-inspector/LICENSE'),'utf8'));
});

test('credits retain each vendored transport and its complete upstream license',async()=>{
  const entries=await cargoCredits(repository,{packages:[...packages,{id:'app',name:'transmog-desktop',source:null}]});
  assert.equal(entries.length,3);
  for(const pkg of packages) {
    const entry=entries.find(entry=>entry.name===pkg.name);
    assert.ok(entry,'Missing '+pkg.name);
    assert.equal(entry.version,pkg.version);
    const license=await readFile(resolve(repository,'third_party/rust-transport/vendor',pkg.name,pkg.name==='quiche'?'COPYING':'LICENSE'),'utf8');
    assert.ok(entry.text.includes(license),'Incomplete license for '+pkg.name);
  }
});

test('credits reject transports with a different source, version, or manifest',async()=>{
  for(const pkg of packages) {
    for(const change of [{source:'registry+https://github.com/rust-lang/crates.io-index'},{version:'0.0.0'},{manifest_path:resolve(repository,'Cargo.toml')}]) {
      await assert.rejects(cargoCredits(repository,{packages:[{...pkg,...change}]}),/Unreviewed transport dependency/);
    }
  }
});

test('credits accept a checkout reached through a directory alias',async()=>{
  const temporary=await mkdtemp(join(tmpdir(),'transmog-credits-'));
  try {
    const alias=join(temporary,'checkout');
    await symlink(repository,alias,process.platform==='win32'?'junction':'dir');
    const entries=await cargoCredits(alias,{packages});
    assert.deepEqual(entries.map(entry=>entry.name),packages.map(pkg=>pkg.name));
  } finally {
    await rm(temporary,{recursive:true,force:true});
  }
});
