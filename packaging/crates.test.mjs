import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const read = p => readFileSync(p, 'utf8');
const names = ['proto', 'kernel', 'view', 'host', 'gateway', 'sessions', ''];
test('only the seven production crates opt into publication, with package metadata', () => {
  for (const suffix of names) {
    const name = `hennery${suffix ? `-${suffix}` : ''}`;
    const manifest = read(`crates/${name}/Cargo.toml`);
    assert.match(manifest, /publish = \["crates-io"\]/);
    for (const key of ['description', 'readme', 'repository', 'homepage']) assert.match(manifest, new RegExp(`^${key} ?[.=]`, 'm'));
    assert.match(read(`crates/${name}/README.md`), /pre-1.0|C compiler/);
  }
  for (const name of ['pins', 'testkit']) assert.match(read(`crates/hennery-${name}/Cargo.toml`), /publish = false/);
  const root = read('Cargo.toml');
  assert.match(root, /publish = false/);
  const version = root.match(/^version = "([^"]+)"$/m)[1];
  for (const name of names.filter(Boolean)) assert.ok(root.includes(`hennery-${name} = { path = "crates/hennery-${name}", version = "=${version}" }`));
});
test('host includes its canonical package-local manifest and generator writes it', () => {
  assert.match(read('crates/hennery-host/src/runtime/manifest.rs'), /include_str!\("\.\.\/\.\.\/manifest.json"\)/);
  assert.match(read('crates/hennery-pins/src/main.rs'), /crates\/hennery-host\/manifest.json/);
  assert.doesNotThrow(() => JSON.parse(read('crates/hennery-host/manifest.json')));
});
test('Cargo source builders use maintained platform OpenSSL', () => {
  assert.doesNotMatch(read('crates/hennery/Cargo.toml'), /vendored-openssl|openssl\/vendored/);
  const phase = execFileSync('nix', ['eval', '--raw', '.#checks.x86_64-linux.package.buildPhase'], { encoding: 'utf8' });
  assert.match(phase, /cargoWithProfile build .*--locked -p hennery --no-default-features/);
});

// Cargo's include entries use gitignore-style matching: an unanchored basename
// also selects matching files below tests/. Exercise Cargo, not a glob imitation.
import fs from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';
test('actual Cargo packages retain production embeds and omit test file collisions', () => {
  const fixtures = fs.mkdtempSync(join(tmpdir(), 'hennery-r3-include-'));
  for (const suffix of names) {
    const name = `hennery${suffix ? `-${suffix}` : ''}`;
    const includes = read(`crates/${name}/Cargo.toml`).match(/^include = (\[.*\])$/m)[1];
    const root = join(fixtures, name);
    const inputs = {
      'src/lib.rs': 'pub mod startup;\n',
      'src/startup.rs': '// production C2 startup code\n',
      'src/delegation_schema.sql': '-- production D2 embed\n',
      'build.rs': 'fn main() {}\n',
      'package_ui.rs': '// production UI build helper\n',
      'manifest.json': '{}\n',
      'README.md': 'Package readme\n',
      'LICENSE': 'Package licence\n',
      'web-dist/index.html': '<html>UI</html>\n',
      'release-ui.json': '{}\n',
      'FRONTEND-NOTICES.txt': 'Frontend notices\n',
      'SOURCE.json': '{}\n',
      'tests/package_ui.rs': '// repository integration harness\n',
      'tests/build.rs': '// repository test harness\n',
      'tests/README.md': 'Test-only readme\n',
      'tests/LICENSE': 'Test-only fixture\n',
      'tests/manifest.json': '{}\n',
      'tests/fixtures/undo_delegation.sql': '-- repository-only fixture\n',
    };
    for (const [name, text] of Object.entries(inputs)) {
      fs.mkdirSync(join(root, name, '..'), { recursive: true });
      fs.writeFileSync(join(root, name), text);
    }
    fs.writeFileSync(join(root, 'Cargo.toml'), `[package]\nname="${name}"\nversion="0.0.0"\nedition="2024"\ninclude=${includes}\n[workspace]\n`);
    const listed = execFileSync('cargo', ['package', '--list', '--offline', '--allow-dirty', '--manifest-path', join(root, 'Cargo.toml')], {encoding:'utf8', stdio:['ignore','pipe','pipe']}).trim().split('\n');
    assert.deepEqual(listed.filter(n => /^(tests|benches|examples)\//.test(n)), [], name);
    for (const input of ['src/startup.rs', 'src/delegation_schema.sql', 'LICENSE']) assert.ok(listed.includes(input), `${name}: missing ${input}`);
    if (name === 'hennery-host') assert.ok(listed.includes('manifest.json'));
    if (name === 'hennery-kernel') for (const input of ['package_ui.rs', 'web-dist/index.html', 'FRONTEND-NOTICES.txt', 'SOURCE.json']) assert.ok(listed.includes(input), `kernel: missing ${input}`);
  }
});
