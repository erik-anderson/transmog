import { readFile, readdir, writeFile } from 'node:fs/promises';
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
export async function buildCredits(uiRoot, output) {
  const repository=resolve(uiRoot,'../../..');
  const metadata=JSON.parse(execFileSync('cargo',['metadata','--format-version','1','--locked','--offline'],{cwd:repository,maxBuffer:32*1024*1024,encoding:'utf8',windowsHide:true}));
  const entries=[];
  const byRepository=new Map();
  for(const pkg of metadata.packages.filter(pkg=>pkg.source)) {
    const files=await notices(dirname(pkg.manifest_path));
    if(files.length&&pkg.repository)byRepository.set(pkg.repository,files);
  }
  for(const pkg of metadata.packages.filter(pkg=>pkg.source)) {
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
  const lock=JSON.parse(await readFile(join(uiRoot,'package-lock.json'),'utf8'));
  for(const [path,pkg] of Object.entries(lock.packages)) {
    if(!path||pkg.dev||!pkg.version)continue;
    const name=path.slice(path.lastIndexOf('node_modules/')+'node_modules/'.length);
    const files=await notices(join(uiRoot,path));
    if(!files.length)throw new Error('Missing bundled license text for '+name);
    entries.push({id:name+'@'+pkg.version,name,version:pkg.version,license:pkg.license??'',text:files.map(file=>file.text).join('\n\n')});
  }
  entries.sort((a,b)=>a.name.localeCompare(b.name)||a.version.localeCompare(b.version));
  await writeFile(output,JSON.stringify({summary:await readFile(join(repository,'THIRD_PARTY_NOTICES.md'),'utf8'),entries}));
}
