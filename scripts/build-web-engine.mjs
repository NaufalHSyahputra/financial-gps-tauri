import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const generated = 'src/lib/drivers/web/generated';
for (const [cmd, args] of [
  ['cargo', ['build', '--locked', '--release', '-p', 'web-engine', '--target', 'wasm32-unknown-unknown']],
  ['wasm-bindgen', ['--target', 'web', '--out-dir', generated, 'target/wasm32-unknown-unknown/release/web_engine.wasm']]
]) {
  const result = spawnSync(cmd, args, { cwd: root, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
const wrapper = new URL(`../${generated}/web_engine.js`, import.meta.url);
// The generated wrapper is checked through its accompanying declaration file.
writeFileSync(wrapper, '// @ts-nocheck\n' + readFileSync(wrapper, 'utf8'));
