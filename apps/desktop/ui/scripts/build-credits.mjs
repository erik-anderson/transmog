import { readFile, readdir, realpath, writeFile } from 'node:fs/promises';
import { dirname, resolve, join } from 'node:path';
import { execFileSync } from 'node:child_process';

const noticeName=/^(?:licen[sc]e|copying|copyright|notice|unlicense|authors|third.?party.?notices)(?:[._-].*)?$/i;
async function notices(root, depth=0, vendored=false) {
  const found=[];
  for(const entry of await readdir(root,{withFileTypes:true})) {
    const path=join(root,entry.name);
    if(entry.isFile()&&noticeName.test(entry.name))found.push({file:path,text:await readFile(path,'utf8')});
    else if(entry.isDirectory()&&depth<12&&(vendored||/^(?:vendor|deps|src|third_party|boringssl|quiche|openssl|zlib|v8|licenses)$/i.test(entry.name)))found.push(...await notices(path,depth+1,vendored||/^(?:vendor|third_party|boringssl|v8)$/i.test(entry.name)));
  }
  return found;
}
export async function cargoCredits(repository, metadata) {
  const sources=new Map(Object.entries(JSON.parse(await readFile(join(repository,'third_party/rust-transport/sources.json'),'utf8'))));
  const packages=[];
  for(const pkg of metadata.packages) {
    const source=sources.get(pkg.name);
    if(source) {
      const manifest=await realpath(resolve(repository,'third_party/rust-transport/vendor',pkg.name,'Cargo.toml'));
      if(pkg.source!==null||pkg.version!==source.version||await realpath(pkg.manifest_path)!==manifest)throw new Error('Unreviewed transport dependency: '+pkg.name);
    }
    if(source||pkg.source)packages.push(pkg);
  }
  const entries=[];
  const byRepository=new Map();
  for(const pkg of packages) {
    const files=await notices(dirname(pkg.manifest_path));
    if(files.length&&pkg.repository)byRepository.set(pkg.repository,files);
  }
  for(const pkg of packages) {
    let files=await notices(dirname(pkg.manifest_path));
    if(pkg.license_file&&!files.some(file=>file.file===pkg.license_file))files.push({file:pkg.license_file,text:await readFile(pkg.license_file,'utf8')});
    if(!files.length)files=byRepository.get(pkg.repository)??[];
    if(!files.length) {
      const fallback=join(repository,'third-party',pkg.name,pkg.version);
      try {
        const source=JSON.parse(await readFile(join(fallback,'source.json'),'utf8'));
        const vcs=JSON.parse(await readFile(join(dirname(pkg.manifest_path),'.cargo_vcs_info.json'),'utf8'));
        if(source.version!==pkg.version||source.package!==pkg.name||source.commit!==vcs.git.sha1)throw new Error('Stale license snapshot');
        files=[{file:join(fallback,'LICENSE'),text:await readFile(join(fallback,'LICENSE'),'utf8')}];
      }catch {throw new Error('Missing or stale bundled license text for '+pkg.name+' '+pkg.version);}
    }
    entries.push({id:pkg.name+'@'+pkg.version,name:pkg.name,version:pkg.version,license:pkg.license??'',text:files.map(file=>file.text).join('\n\n')});
  }
  return entries;
}
export async function extensionCredits(repository) {
  const mapi=JSON.parse(await readFile(join(repository,'third_party/mapi-inspector/source.json'),'utf8'));
  return [{id:'mapi-inspector',name:'Microsoft MAPI protocol enum data',version:mapi.revision,license:'MIT',text:await readFile(join(repository,'third_party/mapi-inspector/LICENSE'),'utf8')}];
}
export async function buildCredits(uiRoot, output) {
  const repository=resolve(uiRoot,'../../..');
  const metadata=JSON.parse(execFileSync('cargo',['metadata','--format-version','1','--locked','--offline'],{cwd:repository,maxBuffer:32*1024*1024,encoding:'utf8',windowsHide:true}));
  const entries=await cargoCredits(repository,metadata);
  entries.push(...await extensionCredits(repository));
  const lock=JSON.parse(await readFile(join(uiRoot,'package-lock.json'),'utf8'));
  for(const [path,pkg] of Object.entries(lock.packages)) {
    if(!path||pkg.dev||!pkg.version)continue;
    const name=path.slice(path.lastIndexOf('node_modules/')+'node_modules/'.length);
    const files=await notices(join(uiRoot,path));
    if(!files.length)throw new Error('Missing bundled license text for '+name);
    entries.push({id:name+'@'+pkg.version,name,version:pkg.version,license:pkg.license??'',text:files.map(file=>file.text).join('\n\n')});
  }
  for(const [directory,name,versionFile,versionField,license] of [
    ['boringssl-pki','BoringSSL PKI','manifest.json','revision','Apache-2.0'],
    ['chromium-suffix-list','Chromium Public Suffix List','snapshot.json','chromiumVersion','MPL-2.0'],
    ['chromium-root-store','Chromium root-store data','snapshot.json','rootStoreVersion','BSD-3-Clause'],
    ['chromium-certificate-policy','Chromium certificate policy data','snapshot.json','chromiumVersion','BSD-3-Clause'],
  ]) {
    const base=join(repository,'third_party',directory);
    const receipt=JSON.parse(await readFile(join(base,versionFile),'utf8'));
    entries.push({id:directory,name,version:String(receipt[versionField]),license,text:await readFile(join(base,'LICENSE'),'utf8')});
  }
  entries.sort((a,b)=>a.name.localeCompare(b.name)||a.version.localeCompare(b.version));
  await writeFile(output,JSON.stringify({summary:'Transmog includes third-party libraries, native code and visualizer extensions. Their license and copyright notices are available below.',entries}));
}
