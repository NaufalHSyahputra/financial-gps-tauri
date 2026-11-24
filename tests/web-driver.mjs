import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { createHash } from 'node:crypto';
import { hranaFixture } from './hrana-fixture.mjs';
import { chromium } from 'playwright';

const origin = 'http://127.0.0.1:1423';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', 'dev', '--host', '127.0.0.1', '--port', '1423', '--strictPort'], { stdio: ['ignore', 'ignore', 'inherit'] });
const temporary = mkdtempSync(join(tmpdir(), 'fgps-worker-test-'));
const auth = 'fixture-token-never-persist-in-clear';
const { endpoint, counts, close } = hranaFixture(origin, join(temporary, 'turso.db'), new Set([auth]));

let browser;
async function host() {
  const context = await browser.newContext();
  await context.route('https://phase-three-test.turso.io/v3/pipeline', endpoint);
  await context.route(`${origin}/driver-harness`, route => route.fulfill({
    contentType: 'text/html',
    headers: {
      'Cross-Origin-Opener-Policy': 'same-origin',
      'Cross-Origin-Embedder-Policy': 'require-corp'
    },
    body: '<!doctype html><title>Worker integration harness</title>'
  }));
  const page = await context.newPage();
  await page.goto(`${origin}/driver-harness`);
  return page;
}

async function boot(page) {
  return page.evaluate(async () => {
    const { WebStandaloneDriver } = await import('/src/lib/drivers/web/web-standalone-driver.ts');
    window.fixture = new WebStandaloneDriver();
    window.ok = async (method, ...args) => {
      const result = await window.fixture.commands[method](...args);
      if (result.status !== 'ok') throw new Error(`${method}: ${JSON.stringify(result.error)}`);
      return result.data;
    };
    return window.ok('appBootstrap');
  });
}

try {
  for (let i = 0; i < 100; i++) {
    if (server.exitCode !== null) throw new Error('Vite failed to start');
    try { if ((await fetch(origin)).ok) break; } catch {}
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH });
  const app = await browser.newPage();
  await app.goto(origin);
  await app.getByRole('heading', { name: 'Your activity' }).waitFor();
  await app.close();
  console.log('PASS standalone launcher initializes without Tauri');

  const a = await host();
  await boot(a);
  const stored = await a.evaluate(async () => {
    const ledger = await window.ok('ledgerCreate', { name: 'West', baseCurrency: 'USD', protection: 'plain', rememberKey: false });
    const scope = ledger.scope;
    const mutation = () => ({ scope, requestId: crypto.randomUUID() });
    const account = (await window.ok('accountSave', mutation(), { draft: { name: 'Café Wallet', kind: 'cash', currencyCode: 'USD' } })).value;
    const category = (await window.ok('categorySave', mutation(), {
      draft: { name: 'Groceries', kind: 'expense', icon: 'coffee' }
    })).value;
    const transaction = await window.ok('transactionSave', mutation(), { draft: {
      accountId: account.id, categoryId: category.id, amount: { currency: 'USD', minor: '1234' },
      description: 'Groceries at the marché', direction: 'debit', status: 'posted', bankDate: '2026-11-23'
    }});
    const search = await window.ok('searchGlobal', scope, { query: 'cafe', types: ['account'], page: { limit: 15 } });
    const rows = await window.ok('transactionsList', scope, { filter: { text: 'marche' }, page: { limit: 15 } });
    const status = await window.ok('ledgerSyncNow', scope);
    return { scope, account, transaction, search, rows, status };
  });
  assert.equal(stored.rows.rows[0].amount.minor, '1234');
  assert.equal(stored.rows.rows[0].category.icon, 'coffee');
  assert.equal(stored.search.rows[0].label, 'Café Wallet');
  assert.equal(stored.status.mode, 'local-isolated');
  assert.equal(counts.requests, 0);
  console.log('PASS shared STRICT schema, FTS5, financial_normalize and zero isolated storage requests');

  await a.reload();
  await boot(a);
  const reopened = await a.evaluate(async id => {
    const list = await window.ok('ledgerList');
    const result = await window.ok('ledgerOpen', { ledgerInstanceId: id, requestId: crypto.randomUUID() });
    const accounts = await window.ok('accountsList', result.session.scope, {});
    window.scope = result.session.scope;
    return { list, accounts, scope: window.scope };
  }, stored.scope.ledgerInstanceId);
  assert.equal(reopened.list.length, 1);
  assert.equal(reopened.accounts[0].account.name, 'Café Wallet');
  assert.equal(reopened.accounts[0].account.kind, 'cash');
  assert.notEqual(reopened.scope.sessionId, stored.scope.sessionId);
  console.log('PASS OPFS ledger data survives a worker and page restart');

  const removed = await a.evaluate(() => ['importPreviewStart', 'fxRefreshStart', 'fxRatesList']
    .every(name => !(name in window.fixture.commands)));
  assert.ok(removed);
  assert.equal(counts.requests, 0);
  console.log('PASS retired commands are absent from the browser driver');

  for (const version of [1, 2, 3]) {
    const legacyPath = join(temporary, `legacy-${version}.db`);
    const schema = readFileSync(new URL('../crates/core-engine/migrations/0001_init.sql', import.meta.url), 'utf8');
    const legacyDb = new DatabaseSync(legacyPath);
    legacyDb.exec(`CREATE TABLE schema_migrations (
            version INTEGER PRIMARY KEY CHECK(version>0),
            sha256 TEXT NOT NULL CHECK(length(sha256)=64),
            applied_at_ms INTEGER NOT NULL
        ) STRICT;`);
    legacyDb.exec(schema);
    legacyDb.prepare('INSERT INTO schema_migrations VALUES(1,?,0)').run(createHash('sha256').update(schema).digest('hex'));
    legacyDb.exec(`INSERT INTO ledger_meta(singleton,ledger_uuid,name,display_currency,created_at_ms)
      VALUES(1,'00000000-0000-4000-8000-000000000002','Old activity','USD',0);
      INSERT INTO accounts(id,name,kind,currency_code) VALUES('original','Old bank','checking','USD');
      INSERT INTO categories(id,name,kind) VALUES('food','Groceries','expense');
      PRAGMA user_version=1;`);
    if (version >= 2) {
      const cash = readFileSync(new URL('../crates/core-engine/migrations/0002_cash_accounts.sql', import.meta.url), 'utf8');
      legacyDb.exec('PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON; BEGIN IMMEDIATE;');
      legacyDb.exec(cash);
      legacyDb.prepare('INSERT INTO schema_migrations VALUES(2,?,0)').run(createHash('sha256').update(cash).digest('hex'));
      legacyDb.exec('PRAGMA user_version=2; COMMIT; PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;');
    }
    if (version === 3) {
      const icons = readFileSync(new URL('../crates/core-engine/migrations/0003_category_icons.sql', import.meta.url), 'utf8');
      legacyDb.exec(icons);
      legacyDb.prepare('INSERT INTO schema_migrations VALUES(3,?,0)').run(createHash('sha256').update(icons).digest('hex'));
      legacyDb.exec('PRAGMA user_version=3;');
    }
    legacyDb.close();
    const legacy = await host();
    await boot(legacy);
    const chooser = legacy.waitForEvent('filechooser');
    const grant = legacy.evaluate(() => window.ok('fileChoose', { purpose: 'ledgerOpen' }));
    await (await chooser).setFiles(legacyPath);
    const [selection] = await grant;
    const savedLegacy = await legacy.evaluate(async openGrant => {
      const opened = await window.ok('ledgerOpen', { openGrant, requestId: crypto.randomUUID() });
      const scope = opened.session.scope;
      const result = await window.ok('accountSave', { scope, requestId: crypto.randomUUID() }, {
        draft: { name: 'Cash from older file', kind: 'cash', currencyCode: 'USD' }
      });
      const [category] = (await window.ok('categoriesList', scope, { page: { limit: 10 } })).rows;
      const updated = await window.ok('categorySave', { scope, requestId: crypto.randomUUID() }, {
        target: { id: category.id, version: category.version },
        draft: { ...category.draft, icon: 'shopping-cart' }
      });
      return { account: result.value, rows: await window.ok('accountsList', scope, {}), category, updated: updated.value };
    }, selection.id);
    assert.equal(savedLegacy.account.account.kind, 'cash');
    assert.ok(savedLegacy.rows.some(item => item.account.name === 'Old bank'));
    await legacy.context().close();
    assert.equal(savedLegacy.category.draft.name, 'Groceries');
    assert.equal(savedLegacy.category.draft.icon ?? null, null);
    assert.equal(savedLegacy.updated.draft.icon, 'shopping-cart');
  }
  console.log('PASS browser upgrades V1 through V3 databases, keeps existing accounts and categories, and saves category icons');

  const url = 'libsql://phase-three-test.turso.io';
  const switching = await a.evaluate(async ({ url, auth }) => {
    const wrong = await window.fixture.secrets.setStorageMode(window.scope, { mode: 'turso-sync', url, authToken: 'invalid-token' });
    const invalid = await window.fixture.secrets.validateTurso({ url: 'https://phase-three-test.turso.io.example.com', authToken: auth });
    const valid = await window.fixture.secrets.setStorageMode(window.scope, { mode: 'turso-sync', url, authToken: auth });
    return { wrong, invalid, valid };
  }, { url, auth });
  assert.equal(switching.wrong.error.messageKey, 'turso_auth_rejected');
  assert.equal(switching.invalid.error.fields.url, 'invalid_turso_endpoint');
  assert.equal(switching.valid.data.mode, 'turso-sync');

  await a.reload();
  await boot(a);
  await a.evaluate(async id => {
    window.scope = (await window.ok('ledgerOpen', { ledgerInstanceId: id, requestId: crypto.randomUUID() })).session.scope;
  }, stored.scope.ledgerInstanceId);
  const encrypted = await a.evaluate(async ({ id, auth }) => {
    const { BrowserRecords, openRecords } = await import('/src/lib/drivers/web/secret-vault.ts');
    const db = await openRecords();
    try {
      const records = new BrowserRecords(db);
      const key = await records.get('vault:key:v1');
      const box = await records.get(`vault:secret:turso:${id}`);
      return !key.extractable && key.algorithm.length === 256 && box.nonce.length === 12 &&
        !new TextDecoder().decode(box.ciphertext).includes(auth) && localStorage.length === 0 && sessionStorage.length === 0;
    } finally { db.close(); }
  }, { id: stored.scope.ledgerInstanceId, auth });
  assert.equal(encrypted, true);
  await a.evaluate(async () => {
    const mutation = () => ({ scope: window.scope, requestId: crypto.randomUUID() });
    await window.ok('financialCalendarSave', mutation(), { timezone: 'Asia/Jakarta', anchor: 10, weekdayPolicy: 'none' });
    const calendar = await window.ok('financialCalendarGet', window.scope);
    await window.ok('cashPlanEdit', mutation(), { save: { name: 'Turso planning fixture', dueOn: calendar.today, minor: '1500', kind: 'obligation' } });
  });
  assert.equal((await a.evaluate(() => window.ok('ledgerSyncNow', window.scope))).remoteGeneration, '1');
  console.log('PASS AES-GCM IndexedDB credential round trip across restart and live SELECT 1 protocol');

  const b = await host();
  await boot(b);
  await b.evaluate(async ({ url, auth }) => {
    window.scope = (await window.ok('ledgerCreate', {
      name: 'East', baseCurrency: 'USD', protection: 'encrypted', credential: 'Browser fixture passphrase', rememberKey: true
    })).scope;
    await window.ok('ledgerStorageModeSet', window.scope, { mode: 'turso-sync', url, authToken: auth });
    await window.ok('ledgerSyncNow', window.scope);
  }, { url, auth });
  const synced = await b.evaluate(() => window.ok('categoriesList', window.scope, { page: { limit: 10 } }));
  assert.equal(synced.rows[0].draft.icon, 'coffee');
  const planning = await b.evaluate(async () => ({
    calendar: await window.ok('financialCalendarGet', window.scope),
    forecast: await window.ok('cashPlanGet', window.scope, { uncertainIds: [] }),
  }));
  assert.equal(planning.calendar.settings.anchor, 10);
  assert.equal(planning.forecast.entries.find(row => row.name === 'Turso planning fixture').amount.minor, '1500');
  console.log('PASS payday calendar and forecast assumptions survive encrypted replica sync');
  counts.networkDisabled = true;
  const prior = counts.requests;
  const makeWrite = (page, name) => page.evaluate(async name => {
    const accounts = await window.ok('accountsList', window.scope, {});
    return window.ok('accountSave', { scope: window.scope, requestId: crypto.randomUUID() }, {
      target: { id: accounts[0].id, version: accounts[0].version },
      draft: { name, kind: accounts[0].account.kind, currencyCode: 'USD' }
    });
  }, name);
  await makeWrite(a, 'West remained local');
  await makeWrite(b, 'East remained local');
  assert.equal(counts.requests, prior);
  const offline = await b.evaluate(() => window.ok('ledgerSyncNow', window.scope));
  assert.equal(offline.phase, 'offline');
  assert.equal(offline.pending, true);
  counts.networkDisabled = false;
  assert.equal((await a.evaluate(() => window.ok('ledgerSyncNow', window.scope))).remoteGeneration, '2');
  const conflict = await b.evaluate(() => window.ok('ledgerSyncNow', window.scope));
  assert.equal(conflict.error.messageKey, 'turso_concurrent_ledger_edits');
  assert.equal(conflict.pending, true);
  const after = await b.evaluate(async generation => {
    const local = (await window.ok('accountsList', window.scope, {}))[0].account.name;
    const resolved = await window.ok('ledgerSyncPublishLocal', window.scope, generation);
    return { local, resolved };
  }, conflict.error.fields.remoteGeneration);
  assert.equal(after.local, 'East remained local');
  assert.equal(after.resolved.remoteGeneration, '3');
  const fromEast = await a.evaluate(async () => {
    await window.ok('ledgerSyncNow', window.scope);
    return (await window.ok('accountsList', window.scope, {}))[0].account.name;
  });
  assert.equal(fromEast, 'East remained local');
  console.log('PASS local-first outbox, category icon sync, and explicit two-device resolution');

  const archiveResult = await a.evaluate(async () => {
    const ticket = await window.ok('backupCreateStart', window.scope, { requestId: crypto.randomUUID(), backupPassphrase: 'Archive fixture password' });
    const wait = async (ticket, scope) => {
      for (let i = 0; i < 120; i++) {
        const status = await window.ok('jobStatus', ticket.id, scope);
        if (status.output) return status.output.value;
        if (status.error) throw new Error(JSON.stringify(status.error));
        await new Promise(resolve => setTimeout(resolve, 30));
      }
      throw new Error('Job timeout');
    };
    const backup = await wait(ticket, window.scope);
    const preview = await wait(await window.ok('restorePreviewStart', {
      scopedBackupId: backup.id, backupPassphrase: 'Archive fixture password'
    }), null);
    const restored = await wait(await window.ok('restoreApplyStart', {
      requestId: crypto.randomUUID(), previewToken: preview.previewToken, protection: 'plain',
      target: { kind: 'new', name: 'Restored' }
    }), null);
    const records = await window.ok('transactionsList', restored.scope, { filter: {}, page: { limit: 10 } });
    const calendar = await window.ok('financialCalendarGet', restored.scope);
    const forecast = await window.ok('cashPlanGet', restored.scope, { uncertainIds: [] });
    return { backup, restored, records, calendar, forecast };
  });
  assert.equal(archiveResult.records.rows[0].amount.minor, '1234');
  assert.equal(archiveResult.records.rows[0].category.icon, 'coffee');
  assert.notEqual(archiveResult.restored.scope.ledgerInstanceId, stored.scope.ledgerInstanceId);
  assert.equal(archiveResult.calendar.settings.anchor, 10);
  assert.ok(archiveResult.forecast.entries.some(row => row.name === 'Turso planning fixture'));
  console.log('PASS encrypted portable browser backup and restore with category icons and financial planning data');
} finally {
  await browser?.close();
  server.kill();
  close();
  rmSync(temporary, { recursive: true, force: true });
}
