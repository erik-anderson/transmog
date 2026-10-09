// Maintenance helper: pin missing crate license texts to their published VCS commits.
// Normal UI builds read these snapshots offline and fail for missing notices.
import {execFileSync} from 'node:child_process';
import {readFile,readdir,mkdir,writeFile} from 'node:fs/promises';
import {dirname,join,resolve} from 'node:path';
const root=resolve(import.meta.dirname,'..');
const metadata=JSON.parse(execFileSync('cargo',['metadata','--format-version','1','--locked','--offline'],{cwd:root,encoding:'utf8',maxBuffer:32000000,windowsHide:true}));
const packages=metadata.packages.filter(pkg=>pkg.source);
const license=/^(?:licen[sc]e|copying|copyright|notice|unlicense|authors)(?:[._-].*)?$/i;
const repositories=new Set();
for(const pkg of packages)if((await readdir(dirname(pkg.manifest_path))).some(name=>license.test(name)))repositories.add(pkg.repository);
const downloaded=new Map();
for(const pkg of packages.filter(pkg=>!repositories.has(pkg.repository))) {
  if(!pkg.repository?.startsWith('https://github.com/'))throw new Error('Unrecognized repository for '+pkg.name);
  const vcs=JSON.parse(await readFile(join(dirname(pkg.manifest_path),'.cargo_vcs_info.json'),'utf8'));
  const repository=pkg.repository.replace(/\/$/,'').replace(/\.git$/,'').slice('https://github.com/'.length);
  const key=repository+':'+vcs.git.sha1;
  let notice=downloaded.get(key);
  if(!notice){
    const response=await fetch('https://api.github.com/repos/'+repository+'/contents/?ref='+vcs.git.sha1,{headers:{'User-Agent':'Transmog-notice-collector'}});
    if(!response.ok)throw new Error('License directory lookup failed for '+key+': '+response.status);
    const listing=await response.json();
    let files=listing.filter(file=>file.type==='file'&&license.test(file.name));
    for(const dir of listing.filter(file=>file.type==='dir'&&/^licen[sc]es$/i.test(file.name))) {
      const response=await fetch(dir.url,{headers:{'User-Agent':'Transmog-notice-collector'}});
      if(!response.ok)throw new Error('License directory unavailable');
      files.push(...(await response.json()).filter(file=>file.type==='file'));
    }
    notice={texts:[],sources:[]};
    if(!files.length) {
      if(pkg.license!=='MIT')throw new Error('No root license files at '+key);
      const declared='https://raw.githubusercontent.com/'+repository+'/'+vcs.git.sha1+'/'+(vcs.path_in_vcs?vcs.path_in_vcs+'/':'')+'Cargo.toml';
      const template=(await readFile(join(root,'LICENSE'),'utf8')).replace(/^Copyright[^\n]*\n/gm,'');
      notice.texts.push('The upstream package declares the MIT license in Cargo.toml and supplies no separate copyright notice.\n\n'+template);
      notice.sources.push(declared);
    }
    for(const file of files){const source='https://raw.githubusercontent.com/'+repository+'/'+vcs.git.sha1+'/'+file.path;const response=await fetch(source);if(!response.ok)throw new Error('License download failed: '+source);notice.texts.push(await response.text());notice.sources.push(source);}
    downloaded.set(key,notice);
  }
  const output=join(root,'third-party',pkg.name,pkg.version);
  await mkdir(output,{recursive:true});await writeFile(join(output,'LICENSE'),notice.texts.join('\n\n'));await writeFile(join(output,'source.json'),JSON.stringify({package:pkg.name,version:pkg.version,commit:vcs.git.sha1,sources:notice.sources},null,2)+'\n');
  console.log('Saved notices for '+pkg.name+' '+pkg.version);
}
