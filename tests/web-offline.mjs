import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { createReadStream, existsSync, statSync } from 'node:fs';
import { extname, resolve, sep } from 'node:path';
import { chromium } from 'playwright';

const root = resolve('build');
const types = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.css': 'text/css', '.wasm': 'application/wasm',
  '.json': 'application/json'
};
const server = createServer((request, response) => {
  const path = resolve(root, '.' + decodeURIComponent(new URL(request.url, 'http://local.test').pathname));
  if (path !== root && !path.startsWith(root + sep)) {
    response.writeHead(404).end();
    return;
  }
  const file = existsSync(path) && statSync(path).isFile() ? path : resolve(root, 'index.html');
  response.setHeader('Content-Type', types[extname(file)] ?? 'application/octet-stream');
  response.setHeader('Content-Security-Policy', "connect-src 'self' https://*.turso.io wss://*.turso.io; object-src 'none'; worker-src 'self'");
  createReadStream(file).pipe(response);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH });
try {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto(origin);
  await page.getByRole('heading', { name: 'Your activity' }).waitFor();
  await page.getByRole('button', { name: 'More setup options' }).click();
  const name = 'Offline production activity';
  await page.getByLabel('Name', { exact: true }).fill(name);
  await page.getByRole('button', { name: 'Create activity', exact: true }).click();
  await page.waitForURL('**/dashboard');
  await page.waitForFunction(() => !!navigator.serviceWorker.controller, { timeout: 20_000 });
  await context.setOffline(true);
  await page.goto(origin);
  await page.getByRole('heading', { name: 'Your activity' }).waitFor();
  assert.match(await page.locator('body').innerText(), /Offline production activity/);
  console.log('PASS production CSP, cached startup and OPFS registry while the browser is offline');
} finally {
  await browser.close();
  server.close();
}
