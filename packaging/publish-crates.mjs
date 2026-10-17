// Exact-byte uploader inside R1's protected, manually approved tagged orchestrator.
import fs from 'node:fs';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { pathToFileURL } from 'node:url';
import { sha256, readJSON, requireThat, verifyCandidate, validateSource, publishBody } from './crate-release.mjs';
import { indexPath } from './crate-registry.mjs';
export async function preflight({dir,digest,gate,fetcher=fetch}) {
  const inventoryFile=join(dir,'crates-inventory.json');
  function validate() {
    requireThat(sha256(fs.readFileSync(inventoryFile))===digest,'inventory changed after approval');
    const inventory=readJSON(inventoryFile);verifyCandidate(dir,inventory);return inventory;
  }
  const inventory=validate();
  const authorize=async(phase='boundary')=>{validate();await gate({channel:'crates',dir,digest,source_sha:inventory.source.sha,version:inventory.version,phase});validate();};
  await authorize('preflight');
  await validateSource(inventory.source,fetcher);
  await authorize('preflight');
  return {inventory,authorize};
}
export async function publishFrozen({dir,digest,gate,credentials,fetcher=fetch,pause=delay}) {
  const {inventory,authorize}=await preflight({dir,digest,gate,fetcher});
  const token=await credentials();requireThat(typeof token==='string' && token.length>0,'publication credential missing');
  const completed=[];
  for(const c of inventory.crates) {
    await authorize();
    const download=`https://crates.io/api/v1/crates/${c.name}/${inventory.version}/download`;
    let remote=await fetcher(download,{signal:AbortSignal.timeout(120000)});
    if(remote.status===404) {
      // Revalidate at the actual credentialed boundary. No repackaging or UI rebuild occurs.
      await authorize();
      const bytes=fs.readFileSync(join(dir,c.file));
      requireThat(sha256(bytes)===c.sha256,'package changed at upload boundary');
      let response;
      try {
        response=await fetcher('https://crates.io/api/v1/crates/new',{method:'PUT',headers:{Authorization:token,'Content-Type':'application/octet-stream','User-Agent':'hennery-release-r3'},body:publishBody(c.upload,bytes),signal:AbortSignal.timeout(120000)});
      } catch {throw new Error(`ambiguous upload for ${c.name}: stop; a newly approved attempt must reconcile remote bytes`);}
      requireThat(response.ok,`upload refused/ambiguous for ${c.name}: HTTP ${response.status}; stop and reconcile in a newly approved attempt`);
      // Fetch final remote bytes after upload. A successful response alone proves nothing.
      remote=await fetcher(download,{signal:AbortSignal.timeout(120000)});
    }
    requireThat(remote.ok,`remote package unavailable/unverifiable: ${c.name}`);
    requireThat(sha256(Buffer.from(await remote.arrayBuffer()))===c.sha256,`remote digest differs: ${c.name}; new signed version required`);
    let resolvable=false;
    for(let attempt=0;attempt<48;attempt++) {
      await authorize();
      const index=await fetcher(`https://index.crates.io/${indexPath(c.name)}`,{cache:'no-store',signal:AbortSignal.timeout(30000)});
      requireThat(index.ok || index.status===404,'registry index unavailable');
      if(index.ok) {
        const entry=(await index.text()).trim().split('\n').map(line=>JSON.parse(line)).find(e=>e.vers===inventory.version);
        if(entry){requireThat(!entry.yanked && entry.cksum===c.sha256,'registry index digest differs/yanked');resolvable=true;break;}
      }
      await pause(10000);
    }
    requireThat(resolvable,`registry did not resolve ${c.name}; stop before consumers`);
    completed.push(c.name);
  }
  return completed;
}
export function releaseCratesDigest(dir) {
 const raw=fs.readFileSync(join(dir,'SHA256SUMS'),'utf8'),entries=new Map(),folded=new Set();
 requireThat(raw.endsWith('\n'),'checksum newline missing');
 for(const line of raw.slice(0,-1).split('\n')){
  const match=/^([a-f0-9]{64})  ([A-Za-z0-9][A-Za-z0-9._-]*)$/.exec(line);
  requireThat(match && !folded.has(match[2].toLowerCase()) && !['sha256sums','release.sigstore.jsonl'].includes(match[2].toLowerCase()),'invalid/duplicate checksum entry');
  entries.set(match[2],match[1]);folded.add(match[2].toLowerCase());
 }
 requireThat([...entries.keys()].join('\n')===[...entries.keys()].sort().join('\n'),'checksum list unsorted');
 const names=fs.readdirSync(dir).filter(n=>n!=='SHA256SUMS').sort();
 requireThat(names.join('\n')===[...entries.keys()].join('\n'),'global checksum closure differs');
 for(const name of names){const path=join(dir,name);requireThat(fs.lstatSync(path).isFile() && !fs.lstatSync(path).isSymbolicLink() && sha256(fs.readFileSync(path))===entries.get(name),'global payload differs: '+name);}
 const root=readJSON(join(dir,'release-metadata.json')),inventory=readJSON(join(dir,'crates-inventory.json'));
 requireThat([2,3].includes(root.schema) && root.repository==='marcinwadon/hennery' && root.channels?.includes('crates') && root.channels.includes('github') && root.source_sha===inventory.source.sha && root.tag===`v${inventory.version}`,'release crate source/tag/channel differs');
 requireThat(inventory.source.url===`https://github.com/marcinwadon/hennery/releases/download/${root.tag}/hennery-source-${root.source_sha}.tar.gz`,'release source URL differs');
 verifyCandidate(dir,inventory);
 return entries.get('crates-inventory.json');
}
let gateSequence=0;
export function gateBroker() {
 const request=process.env.CRATES_GATE_REQUEST_FD,reply=process.env.CRATES_GATE_REPLY_FD;
 requireThat(/^[1-9][0-9]*$/.test(request||'') && /^[1-9][0-9]*$/.test(reply||'') && request!==reply,'private release gate broker missing');
 const write=Number(request),read=Number(reply);
 requireThat(Number.isSafeInteger(write) && Number.isSafeInteger(read) && write>=3 && read>=3
   && fs.fstatSync(write).isFIFO() && fs.fstatSync(read).isFIFO(),'invalid private release gate pipes');
 const sequence=++gateSequence,message=Buffer.from(`gate-v1:${sequence}\n`);
 requireThat(fs.writeSync(write,message)===message.length,'incomplete release gate request');
 const byte=Buffer.alloc(1);let response='';
 while(response.length<128){
  requireThat(fs.readSync(read,byte)===1,'release gate broker EOF/refusal');
  response+=byte.toString('latin1');if(response.endsWith('\n'))break;
 }
 const match=/^ok-v1:([1-9][0-9]*):([0-9a-f]{64})\n$/.exec(response);
 requireThat(match && match[1]===String(sequence),'release gate broker reply mismatch');
 return match[2];
}
export function releaseGate({dir}) {
 const before=releaseCratesDigest(dir),approved=gateBroker();
 requireThat(approved===before && releaseCratesDigest(dir)===approved,'release crate manifest changed at parent gate');
 return approved;
}

if(import.meta.url===pathToFileURL(process.argv[1]||'').href) {
 requireThat(process.env.GITHUB_ACTIONS==='true','real publisher only runs in the protected tagged workflow');
 const verifyOnly=process.argv[2]==='--verify',dir=resolve(process.argv[verifyOnly?3:2]),signatures=resolve(process.argv[verifyOnly?4:3]);
 const digest=releaseCratesDigest(dir),gate=async()=>{requireThat(releaseGate({dir,signatures})===digest,'release crate manifest changed');};
 if(verifyOnly)await preflight({dir,digest,gate});
 else await publishFrozen({dir,digest,gate,credentials:async()=>process.env.CARGO_REGISTRY_TOKEN});
}
