// R3: pure checks shared by preparation, acceptance and the publisher.
import fs from 'node:fs';
import { join, relative } from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
export const ORDER = ['hennery-proto','hennery-kernel','hennery-view','hennery-host','hennery-gateway','hennery-sessions','hennery'];
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
export const readJSON = path => JSON.parse(fs.readFileSync(path,'utf8'));
export function requireThat(ok, message) { if (!ok) throw new Error(message); }
const sameFiles = (a,b) => a && b && Object.keys(a).length===Object.keys(b).length && Object.entries(a).every(([k,v])=>Object.hasOwn(b,k) && b[k]===v);
const hex = (s,n) => typeof s === 'string' && new RegExp(`^[a-f0-9]{${n}}$`).test(s);
export function files(root, prefix='') {
  const result=Object.create(null);
  for (const entry of fs.readdirSync(join(root,prefix),{withFileTypes:true}).sort((a,b)=>a.name.localeCompare(b.name))) {
    const name=prefix ? `${prefix}/${entry.name}` : entry.name;
    requireThat(!entry.isSymbolicLink(),'symlink in distributable: '+name);
    if(entry.isDirectory()) Object.assign(result,files(root,name));
    else { requireThat(entry.isFile(),'non-regular distributable: '+name); result[name]=sha256(fs.readFileSync(join(root,name))); }
  }
  return result;
}
// Compare against Git's complete tree, independently of `git archive`: an
// export-ignore attribute must not silently omit corresponding source/fixtures.
export function gitSourceFiles(revision,cwd=process.cwd()) {
  const entries=execFileSync('git',['ls-tree','-rz','--full-tree',revision],{cwd,encoding:'utf8'}).split('\0').filter(Boolean);
  const result=Object.create(null);
  for(const entry of entries) {
    const tab=entry.indexOf('\t'), [mode,type,oid]=entry.slice(0,tab).split(' '), path=entry.slice(tab+1);
    requireThat(type==='blob' && ['100644','100755'].includes(mode),'source contains unsupported linked/submodule input: '+path);
    result[path]=sha256(execFileSync('git',['cat-file','blob',oid],{cwd,maxBuffer:100*1024*1024}));
  }
  return result;
}
export function validateSource(source) {
  requireThat(hex(source?.sha,40),'source revision must be immutable');
  requireThat(source.url===`https://github.com/marcinwadon/hennery/tree/${source.sha}`,'invalid or mutable corresponding-source URL');
}
export function inspectPackage(root,name,source) {
  requireThat(fs.readFileSync(join(root,'LICENSE'),'utf8')===fs.readFileSync(new URL('../LICENSE',import.meta.url),'utf8'),`${name}: missing or wrong licence text`);
  const all=files(root);
  for(const file of Object.keys(all)) requireThat(!file.startsWith('tests/'),'repository test harness in package: '+file);
  for(const file of Object.keys(all)) requireThat(!/(^|\/)(node_modules|\.npmrc|\.env|target|\.git)(\/|$)|\.map$/.test(file),'forbidden package input: '+file);
  if(name!=='hennery-kernel') return;
  validateSource(source);
  const recorded=readJSON(join(root,'SOURCE.json'));
  requireThat(JSON.stringify(recorded)===JSON.stringify(source),'package source notice differs from candidate');
  requireThat(fs.readFileSync(join(root,'README.md'),'utf8').includes(source.url),'README lacks immutable corresponding-source link');
  const notices=fs.readFileSync(join(root,'FRONTEND-NOTICES.txt'),'utf8');
  requireThat(notices.length>60 && /copyright/i.test(notices) && /license|licence/i.test(notices),'frontend licence texts missing');
  requireThat(hex(source.notices_sha256,64),'frontend notice inventory digest missing');
  requireThat(sha256(Buffer.from(notices))===source.notices_sha256,'frontend notices differ from preparation');
  const marker=readJSON(join(root,'release-ui.json'));
  requireThat(marker.schema===1 && marker.source_sha===source.sha,'UI source revision differs');
  const actual=files(join(root,'web-dist'));
  requireThat(actual['index.html'] && sameFiles(actual,marker.files),'UI inventory differs');
  requireThat(!fs.readFileSync(join(root,'web-dist/index.html'),'utf8').includes('hennery-web-ui-not-embedded'),'placeholder UI');
}
export function inspectProductionFeatures(tree) {
  requireThat(typeof tree==='string' && tree.trim().length>0 && !/\btest-hooks\b|\bhennery-(?:testkit|pins)\b/.test(tree),'test-only production dependency or missing feature evidence');
  requireThat(!/\bopenssl-src\b/.test(tree),'unreviewed bundled OpenSSL in production dependency graph');
}
export function checkSourceTree(root, expected) {
  const actual=files(root);
  for(const path of ['Cargo.toml','Cargo.lock','LICENSE','web/package.json','web/pnpm-lock.yaml','web/vite.config.ts','crates/hennery-kernel/build.rs','crates/hennery-host/manifest.json']) requireThat(actual[path],'incomplete source: '+path);
  requireThat(Object.keys(actual).some(p=>p.startsWith('web/src/')) && Object.keys(actual).some(p=>p.startsWith('crates/hennery/src/')),'incomplete Rust/web source');
  requireThat(sameFiles(actual,expected),'corresponding source does not match source inventory');
}
export function validateInventory(i) {
  requireThat(i?.schema===1 && /^\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(i.version),'invalid candidate version/schema');
  validateSource(i.source);
  requireThat(Array.isArray(i.crates) && i.crates.map(c=>c.name).join()===ORDER.join(),'invalid publish graph/order');
  for(const c of i.crates) {
    requireThat(c.file===`${c.name}-${i.version}.crate` && hex(c.sha256,64),'invalid package filename/digest');
    requireThat(c.size>0 && c.size<=10_000_000,'crate exceeds registry size limit');
    requireThat(c.upload?.name===c.name && c.upload?.vers===i.version,'invalid upload metadata');
  }
}
export function verifyCandidate(dir,i) {
  validateInventory(i);
  for(const c of i.crates) {
    const bytes=fs.readFileSync(join(dir,c.file));
    requireThat(bytes.length===c.size && sha256(bytes)===c.sha256,'candidate bytes differ: '+c.name);
  }
}
export function extract(archive,dest) {
  const names=execFileSync('tar',['-tzf',archive],{encoding:'utf8'}).trim().split('\n');
  for(const n of names) requireThat(n && !n.startsWith('/') && !n.split('/').includes('..'),'unsafe archive path');
  const verbose=execFileSync('tar',['-tvzf',archive],{encoding:'utf8'}).trim().split('\n');
  for(const n of verbose) requireThat(n.startsWith('-')||n.startsWith('d'),'archive contains links or special files');
  fs.mkdirSync(dest,{recursive:true});
  execFileSync('tar',['-xzf',archive,'-C',dest]);
}
export function indexEntry(pkg,digest) {
  return {name:pkg.name,vers:pkg.version,cksum:digest,yanked:false,v:2,rust_version:pkg.rust_version,
    features:{},features2:pkg.features,
    deps:pkg.dependencies.filter(d=>d.kind!=='dev' || d.path===undefined).map(d=>({name:d.rename||d.name,package:d.rename?d.name:undefined,req:d.req,features:d.features,optional:d.optional,default_features:d.uses_default_features,target:d.target,kind:d.kind||'normal',registry:null}))};
}
export function uploadMetadata(pkg) {
  return {name:pkg.name,vers:pkg.version,description:pkg.description,license:pkg.license,readme:pkg.readme?fs.readFileSync(pkg.readme,'utf8'):null,
    repository:pkg.repository,homepage:pkg.homepage,documentation:pkg.documentation,keywords:pkg.keywords,categories:pkg.categories,links:pkg.links,rust_version:pkg.rust_version,features:pkg.features,
    deps:pkg.dependencies.filter(d=>d.kind!=='dev'||d.path===undefined).map(d=>({name:d.name,explicit_name_in_toml:d.rename,version_req:d.req,features:d.features,optional:d.optional,default_features:d.uses_default_features,target:d.target,kind:d.kind||'normal',registry:null}))};
}
export function publishBody(metadata,bytes) {
  const json=Buffer.from(JSON.stringify(metadata)), a=Buffer.alloc(4), b=Buffer.alloc(4);
  a.writeUInt32LE(json.length); b.writeUInt32LE(bytes.length);
  return Buffer.concat([a,json,b,bytes]);
}
