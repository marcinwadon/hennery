// Use the pinned cargo-dist binary: publishable generator crates must never ship installers.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
const plan=JSON.parse(execFileSync(process.env.R3_DIST || 'dist',['plan','--output-format=json'],{encoding:'utf8',maxBuffer:16*1024*1024}));
assert.equal(plan.dist_version,'0.30.4');
assert.deepEqual(plan.releases.map(r=>r.app_name),['hennery']);
const targets=['aarch64-apple-darwin','aarch64-unknown-linux-musl','x86_64-unknown-linux-musl'];
const archives=Object.values(plan.artifacts).filter(a=>a.kind==='executable-zip');
assert.deepEqual(archives.map(a=>a.name).sort(),targets.map(t=>`hennery-${t}.tar.xz`).sort());
for(const archive of archives)assert.deepEqual(archive.assets.filter(a=>a.kind==='executable').map(a=>a.name),['hennery']);
assert.deepEqual(Object.values(plan.artifacts).filter(a=>a.kind==='installer').map(a=>a.name).sort(),['hennery-installer.sh','hennery.rb']);
// A workspace feature list applies to every selected package. Keep CLI-only features local.
assert.doesNotMatch(readFileSync('dist-workspace.toml','utf8'),/^features\s*=/m);
const metadata=JSON.parse(execFileSync('cargo',['metadata','--no-deps','--format-version=1','--locked'],{encoding:'utf8'}));
assert.deepEqual(metadata.packages.find(p=>p.name==='hennery').metadata.dist.features,[]);
console.log('cargo-dist 0.30.4: only hennery; 3 archives, 2 installers; CLI-only features');
