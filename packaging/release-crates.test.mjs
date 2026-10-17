import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {join} from 'node:path';
import {tmpdir} from 'node:os';
import {releaseCratesDigest} from './publish-crates.mjs';
import {ORDER,sha256} from './crate-release.mjs';
function fixture(){
 const dir=fs.mkdtempSync(join(tmpdir(),'release-crates-'));
 const source={sha:'a'.repeat(40),sha256:sha256(Buffer.from('source')),url:`https://github.com/marcinwadon/hennery/releases/download/v0.1.0/hennery-source-${'a'.repeat(40)}.tar.gz`};
 fs.writeFileSync(join(dir,`hennery-source-${source.sha}.tar.gz`),'source');
 const crates=ORDER.map(name=>{const bytes=Buffer.from(name),file=`${name}-0.1.0.crate`;fs.writeFileSync(join(dir,file),bytes);return {name,file,sha256:sha256(bytes),size:bytes.length,upload:{name,vers:'0.1.0'}};});
 fs.writeFileSync(join(dir,'crates-inventory.json'),JSON.stringify({schema:1,version:'0.1.0',source,crates}));
 fs.writeFileSync(join(dir,'release-metadata.json'),JSON.stringify({schema:3,repository:'marcinwadon/hennery',source_sha:source.sha,tag:'v0.1.0',channels:['crates','github']}));
 const sums=()=>fs.readdirSync(dir).filter(n=>n!=='SHA256SUMS').sort().map(n=>sha256(fs.readFileSync(join(dir,n)))+'  '+n+'\n').join('');
 fs.writeFileSync(join(dir,'SHA256SUMS'),sums());return {dir,crates,sums};
}
test('global checksums bind the frozen crate manifest and every tarball',()=>{
 const f=fixture();assert.equal(releaseCratesDigest(f.dir),sha256(fs.readFileSync(join(f.dir,'crates-inventory.json'))));
});
for(const mutation of ['crate','source','extra','missing','duplicate','case-collision','metadata'])test(`release binding refuses ${mutation}`,()=>{
 const f=fixture(),sum=join(f.dir,'SHA256SUMS');
 if(mutation==='crate')fs.appendFileSync(join(f.dir,f.crates[0].file),'x');
 if(mutation==='source')fs.appendFileSync(join(f.dir,`hennery-source-${'a'.repeat(40)}.tar.gz`),'x');
 if(mutation==='extra')fs.writeFileSync(join(f.dir,'extra'),'x');
 if(mutation==='missing')fs.unlinkSync(join(f.dir,f.crates[0].file));
 if(mutation==='duplicate')fs.appendFileSync(sum,fs.readFileSync(sum,'utf8').split('\n')[0]+'\n');
 if(mutation==='case-collision')fs.appendFileSync(sum,fs.readFileSync(sum,'utf8').split('\n')[0].toUpperCase()+'\n');
 if(mutation==='metadata'){const p=join(f.dir,'release-metadata.json'),m=JSON.parse(fs.readFileSync(p));m.source_sha='b'.repeat(40);fs.writeFileSync(p,JSON.stringify(m));fs.writeFileSync(sum,f.sums());}
 assert.throws(()=>releaseCratesDigest(f.dir));
});
