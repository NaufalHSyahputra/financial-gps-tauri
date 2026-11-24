import { readdirSync, readFileSync } from 'node:fs';

const base = new URL('../src/', import.meta.url);
const native = new URL('lib/drivers/native/', base);
const generatedWasm = new URL('lib/drivers/web/generated/', base);
const runtime = new URL('lib/drivers/runtime.ts', base);
const workerTransport = new Set(['db.worker.ts', 'web-standalone-driver.ts']
  .map(file => new URL(`lib/drivers/web/${file}`, base).href));
const tauri = /@tauri-apps\/|(?:[./]bindings(?:['".\/]|$))/;
const invoke = /\binvoke\s*\(/;
let count = 0;
const violations = [];
function check(dir) {
  if (dir.href === native.href || dir.href === generatedWasm.href) return;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const url = new URL(entry.name + (entry.isDirectory() ? '/' : ''), dir);
    if (entry.isDirectory()) check(url);
    else if (/\.(?:ts|svelte|js)$/.test(entry.name)) {
      count++;
      const text = readFileSync(url, 'utf8');
      if (tauri.test(text) ||
        (url.href !== runtime.href && text.includes('__TAURI_INTERNALS__')) ||
        (!workerTransport.has(url.href) && invoke.test(text))) violations.push(url.pathname);
    }
  }
}
check(base);
if (violations.length) {
  console.error(violations.join('\n'));
  process.exitCode = 1;
} else {
  console.log(`Audited ${count} frontend files. 0 direct Tauri imports or IPC references.`);
}

const workspace = new URL('../', import.meta.url);
const source = path => readFileSync(new URL(path, workspace), 'utf8');
const registry = [...source('src/lib/drivers/operations.ts').matchAll(/: \['(\w+)'/g)].map(match => match[1]);
const routes = new Set([
  'file_choose',
  ...[...source('crates/core-engine/src/commands.rs').matchAll(/scoped_(?:read|write)!\(\s*(\w+)/g)].map(match => match[1]),
  ...[...source('crates/web-engine/src/extras.rs').matchAll(/"(\w+)"\s*=>/g)].map(match => match[1]),
  ...[...source('src/lib/drivers/web/db.worker.ts').matchAll(/case '(\w+)':/g)].map(match => match[1])
]);
const absent = registry.filter(operation => !routes.has(operation));
if (absent.length) {
  console.error(`Worker operation routing is missing: ${absent.join(', ')}`);
  process.exitCode = 1;
} else {
  console.log(`All ${registry.length} generated driver operations have a browser route.`);
}
