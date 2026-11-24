import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { hranaFixture } from './hrana-fixture.mjs';

const serverMode = process.env.FINANCIAL_GPS_STORAGE_UI_PREVIEW === '1' ? 'preview' : 'dev';
const port = serverMode === 'preview' ? '1425' : '1424';
const origin = 'http://127.0.0.1:' + port;
const host = 'phase-four-test.turso.io';
const token = 'phase-four-auth-fixture-secret';
const rotated = 'phase-four-rotated-fixture-secret';
const temporary = mkdtempSync(join(tmpdir(), 'fgps-storage-ui-'));
const tokens = new Set([token, rotated]);
const { endpoint, counts, close } = hranaFixture(origin, join(temporary, 'turso.db'), tokens);
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', serverMode, '--host', '127.0.0.1', '--port', port, '--strictPort'], {
  stdio: ['ignore', 'ignore', 'inherit']
});
let browser;
let lastPage;

const nav = (page, name) => page.getByRole('navigation', { name: 'Main navigation' }).getByRole('link', { name, exact: true });
const storage = page => page.getByRole('region', { name: 'Storage & Sync' });
const labelValue = (page, title) => storage(page).getByText(title, { exact: true }).locator('xpath=following-sibling::dd');
const expectText = (locator, value) => locator.filter({ hasText: value }).waitFor();
const button = (root, name) => root.getByRole('button', { name, exact: true });
const authError = /Turso rejected the token/;

async function newDevice(advanced = true, contextOptions = {}, beforeLoad) {
  const context = await browser.newContext({ acceptDownloads: true, ...contextOptions });
  const external = [];
  context.on('request', request => {
    if (/^https?:/.test(request.url()) && new URL(request.url()).origin !== origin) external.push(request.url());
  });
  await context.route(`https://${host}/v3/pipeline`, endpoint);
  await context.addInitScript(() => { window.showSaveFilePicker = undefined; });
  if (beforeLoad) await context.addInitScript(beforeLoad);
  const page = await context.newPage();
  lastPage = page;
  page.setDefaultTimeout(20000);
  await page.goto(origin);
  await page.getByRole('heading', { name: 'Your activity' }).waitFor();
  if (advanced) await button(page, 'More setup options').click();
  assert.equal(await page.evaluate(() => typeof window.__TAURI_INTERNALS__), 'undefined');
  return { page, external };
}

async function credentials(root, secret) {
  await root.getByLabel('Turso Database URL').fill(`libsql://${host}`);
  await root.getByLabel('Turso Auth Token').fill(secret);
}

async function createAccount(page) {
  await nav(page, 'Accounts').click();
  await page.getByRole('link', { name: 'New account' }).click();
  await button(page, 'Continue').click();
  await page.getByLabel('Account name').fill('Travel Wallet');
  await button(page, 'Continue').click();
  await button(page, 'Save account').click();
  try {
    await page.getByRole('heading', { name: 'Travel Wallet' }).waitFor();
  } catch (error) {
    console.error(await page.locator('main').innerText());
    throw error;
  }
}

async function createTransaction(page, description) {
  await nav(page, 'Transactions').click();
  await page.getByRole('link', { name: 'New transaction' }).click();
  await page.getByLabel('Amount (USD)', { exact: true }).fill('12.34');
  await button(page, 'Continue').click();
  const account = page.getByRole('combobox', { name: /^Account/ });
  await account.selectOption({ label: 'Travel Wallet · USD' });
  await button(page, 'Add details').click();
  await page.getByLabel('Description (optional)').fill(description);
  await button(page, 'Save Transaction').click();
  await page.getByRole('heading', { name: description }).waitFor();
}

async function settings(page) {
  await nav(page, 'Settings').click();
  await button(storage(page), 'Switch Mode').waitFor();
}

async function sync(page) {
  await button(storage(page), 'Sync Now').click();
  const finished = storage(page).getByText(/Sync complete\.|Edits on both devices need review\./);
  await finished.first().waitFor();
}

async function search(page, term, expected) {
  await nav(page, 'Transactions').click();
  await page.getByRole('searchbox', { name: 'Search' }).fill(term);
  await button(page, 'Apply search').click();
  await page.getByText(expected, { exact: true }).waitFor();
}

async function verifyTokenAbsent(page) {
  const visible = await page.evaluate(() => JSON.stringify({
    html: document.body.innerHTML,
    formValues: Array.from(document.querySelectorAll('input'), input => input.value),
    local: { ...localStorage },
    session: { ...sessionStorage }
  }));
  assert.ok(!visible.includes(token) && !visible.includes(rotated));
}

async function guidedSetup(currency, starting, formatted) {
  const { page, external } = await newDevice(false);
  const primary = page.getByLabel('Primary currency');
  assert.equal(await primary.inputValue(), 'IDR');
  await primary.selectOption(currency);
  await button(page, 'Continue').click();
  await page.getByRole('radio', { name: /Cash/ }).check();
  const name = page.getByLabel('Account name');
  assert.equal(await name.getAttribute('placeholder'), 'Cash Wallet');
  await name.fill('Daily Cash');
  await button(page, 'Continue').click();
  const balance = page.getByLabel(/Starting balance/);
  if (currency === 'IDR') {
    await balance.fill('12500');
    await page.getByRole('definition').filter({ hasText: 'IDR 12.500' }).waitFor();
  }
  await balance.fill(starting);
  await page.getByRole('definition').filter({ hasText: formatted }).waitFor();
  await button(page, 'Start Tracking').click();
  await page.getByRole('heading', { name: 'Dashboard' }).waitFor();
  await nav(page, 'Accounts').click();
  await page.getByRole('heading', { name: 'Cash', exact: true }).waitFor();
  const cash = page.getByRole('link', { name: /Daily Cash/ });
  await cash.getByText(formatted, { exact: true }).waitFor();
  assert.deepEqual(external, []);
  return page;
}

async function verifyWizards() {
  const idr = await guidedSetup('IDR', '', 'IDR 0');
  await idr.context().close();
  const page = await guidedSetup('USD', '100.50', 'USD 100.50');
  await nav(page, 'Transactions').click();
  await page.getByRole('link', { name: 'New transaction' }).click();
  assert.equal(await button(page, 'Multiply').count(), 0);
  await button(page, 'Continue').click();
  await page.getByRole('alert').filter({ hasText: 'Enter an amount' }).waitFor();
  const amount = page.getByLabel('Amount (USD)');
  await amount.fill('0');
  await button(page, 'Continue').click();
  await page.getByRole('alert').filter({ hasText: 'greater than zero' }).waitFor();
  await button(page, 'Calculator').click();
  await amount.fill('5/0');
  await button(page, 'Equals').click();
  await page.getByRole('alert').filter({ hasText: 'divide by zero' }).waitFor();
  await button(page, 'Clear').click();
  await button(page, '2').click();
  await button(page, 'Add').click();
  await amount.focus();
  await amount.press('End');
  await amount.pressSequentially('1*3');
  await amount.press('=');
  assert.equal(await amount.inputValue(), '5.00');
  await amount.press('Enter');
  await page.getByText('Step 2 of 3').waitFor();
  await button(page, 'Add details').click();
  assert.match(await page.getByRole('definition').allTextContents().then(a => a.join(' ')), /Daily Cash/);
  await button(page, 'Back').click();
  await button(page, 'Save Transaction').click();
  await page.getByRole('heading', { name: 'Expense' }).waitFor();

  await nav(page, 'Accounts').click();
  await page.getByRole('link', { name: 'New account' }).click();
  await button(page, 'Continue').click();
  await page.getByRole('radio', { name: /Bank account/ }).check();
  await page.getByLabel('Account name').fill('Main Bank');
  await button(page, 'Continue').click();
  await button(page, 'Save account').click();
  await page.getByRole('heading', { name: 'Main Bank' }).waitFor();

  await nav(page, 'Transactions').click();
  await page.getByRole('link', { name: 'New transaction' }).click();
  await page.getByRole('tab', { name: 'Transfer', exact: true }).click();
  await page.getByLabel('Amount (USD)').fill('2.00');
  await button(page, 'Continue').click();
  await button(page, 'Save Transaction').click();
  await page.getByRole('alert').filter({ hasText: 'Choose an account' }).waitFor();
  await page.getByRole('combobox', { name: /^From account/ }).selectOption({ label: 'Daily Cash · USD' });
  await page.getByRole('combobox', { name: /^To account/ }).selectOption({ label: 'Main Bank · USD' });
  await button(page, 'Add details').click();
  await page.getByLabel('Description (optional)').fill('Cash to bank');
  await page.getByLabel('Notes (optional)').fill('Moved money for later');
  await button(page, 'Save Transaction').click();
  await page.getByRole('heading', { name: 'Cash to bank' }).waitFor();
  await page.getByRole('heading', { name: 'Linked transfer' }).waitFor();
  await nav(page, 'Categories').click();
  for (const name of ['Food', 'Transport']) {
    await button(page, 'New category').click();
    const dialog = page.getByRole('dialog', { name: 'New category' });
    await dialog.getByLabel('Name').fill(name);
    await button(dialog, 'Save category').click();
    await page.getByRole('cell', { name, exact: true }).waitFor();
  }
  await nav(page, 'Settings').click();
  await page.getByRole('checkbox', { name: 'Shared groups' }).check();
  await button(page, 'Save app preferences').click();
  await nav(page, 'Shared groups').click();
  await button(page, 'New group').click();
  const group = page.getByRole('dialog', { name: 'New shared group' });
  await group.getByLabel('Name', { exact: true }).fill('Friends');
  await button(group, 'Save group').click();
  await page.getByRole('heading', { name: 'Friends' }).waitFor();
  await nav(page, 'Transactions').click();
  await page.getByRole('link', { name: 'New transaction' }).click();
  await page.getByLabel('Amount (USD)').fill('10');
  await button(page, 'Continue').click();
  await page.getByRole('combobox', { name: /^Account/ }).selectOption({ label: 'Daily Cash · USD' });
  await page.getByLabel('Find a category').fill('foo');
  await button(page, 'Food').click();
  assert.equal(await button(page, 'Transport').count(), 0);
  await button(page, 'Add details').click();
  await page.getByLabel('Description (optional)').fill('Trip costs');
  const date = page.getByLabel('Date', { exact: true });
  assert.match(await date.inputValue(), /^\d{4}-\d{2}-\d{2}$/);
  await page.getByRole('combobox', { name: /^Shared group/ }).selectOption({ label: 'Friends' });
  await button(page, 'Split between categories').click();
  await button(page, 'Split between categories').click();
  await page.getByRole('combobox', { name: /^Category for part 1/ }).selectOption({ label: 'Food' });
  await page.getByRole('combobox', { name: /^Category for part 2/ }).selectOption({ label: 'Transport' });
  await page.getByLabel('Amount for part 1').fill('4');
  await page.getByLabel('Amount for part 2').fill('5');
  await button(page, 'Save Transaction').click();
  await page.getByRole('alert').filter({ hasText: 'add up to the full amount' }).waitFor();
  await page.getByLabel('Amount for part 2').fill('6');
  await button(page, 'Save Transaction').click();
  await page.getByRole('heading', { name: 'Trip costs' }).waitFor();
  await nav(page, 'Shared groups').click();
  await page.getByRole('link', { name: /Friends/ }).click();
  await page.getByText('Trip costs', { exact: true }).waitFor();
  await nav(page, 'Settings').click();
  assert.equal(await page.getByRole('link', { name: /currencies|exchange rates|^Import$/i }).count(), 0);
  for (const path of ['/import', '/import/history', '/settings/currencies']) {
    const response = await page.request.get(origin + path);
    assert.equal(response.status(), 404);
  }
  await page.context().close();
  console.log('PASS default IDR and alternate USD setup, Cash balances, calculator input and validation, quick save, linked transfers, split validation, and group assignment');
}

// Observe violations before the launcher loads, including when overlays portal to body.
async function glassDevice(width, height, flavor) {
  const result = await newDevice(false, {
    viewport: { width, height }, hasTouch: width < 768,
    colorScheme: flavor === 'mocha' ? 'dark' : 'light'
  }, () => {
    window.glassCspViolations = [];
    document.addEventListener('securitypolicyviolation', event => {
      window.glassCspViolations.push(event.violatedDirective + ': ' + event.blockedURI);
    });
  });
  assert.equal(await result.page.locator('html').getAttribute('data-theme'), flavor);
  return result;
}

async function glassPanel(locator, dimChrome = false) {
  await locator.waitFor();
  await locator.evaluate(el => Promise.all(el.getAnimations().map(animation => animation.finished.catch(() => {}))));
  const { bounds, filter, clip, maxLayers, barFilters, alpha } = await locator.evaluate(el => {
    const bounds = el.getBoundingClientRect();
    const visible = node => node.getClientRects().length > 0 && getComputedStyle(node).visibility !== 'hidden';
    const filterOf = node => getComputedStyle(node).backdropFilter;
    const sample = document.createElement('canvas').getContext('2d');
    sample.fillStyle = getComputedStyle(el).backgroundColor;
    sample.fillRect(0, 0, 1, 1);
    return {
      alpha: sample.getImageData(0, 0, 1, 1).data[3],
      bounds: { left: bounds.left, top: bounds.top, right: bounds.right, bottom: bounds.bottom },
      filter: filterOf(el),
      clip: { width: innerWidth, height: innerHeight },
      maxLayers: Array.from(document.querySelectorAll('body *')).filter(node => visible(node) && filterOf(node) !== 'none').length,
      barFilters: Array.from(document.querySelectorAll('.liquid-glass-bar')).filter(visible).map(filterOf)
    };
  });
  assert.ok(await locator.evaluate(el => el.classList.contains('liquid-glass-overlay')));
  assert.ok(filter.includes('blur(24px)'), filter);
  assert.ok(alpha > 200 && alpha < 255, 'Overlay should have a translucent fill');
  assert.ok(bounds.left >= -1 && bounds.top >= -1 && bounds.right <= clip.width + 1 && bounds.bottom <= clip.height + 1, JSON.stringify({ bounds, clip }));
  assert.ok(maxLayers <= 2, 'Visible backdrop filters: ' + maxLayers);
  if (dimChrome) assert.ok(barFilters.every(filter => filter === 'none'), 'Chrome should use solid material behind a panel');
}

async function glassCsp(page) {
  assert.deepEqual(await page.evaluate(() => window.glassCspViolations), []);
}

async function verifyMobile(width, height, flavor) {
  const { page, external } = await glassDevice(width, height, flavor);
  async function noOverflow(path) {
    const geometry = await page.evaluate(() => ({ document: document.documentElement.scrollWidth, viewport: innerWidth }));
    assert.ok(geometry.document <= geometry.viewport, `${width}×${height} ${path} overflows (${geometry.document} > ${geometry.viewport})`);
  }
  const tabs = page.getByRole('navigation', { name: 'Mobile navigation' });
  const tab = name => tabs.getByRole('link', { name, exact: true });
  const add = tab('New transaction');
  const wizard = page.getByRole('region', { name: /Enter the amount|Choose an account|Add details/ });
  await noOverflow('/');
  await button(page, 'Continue').click();
  await page.getByRole('radio', { name: /^Cash/ }).check();
  await page.getByLabel('Account name').fill('Mobile Cash');
  await button(page, 'Continue').click();
  await page.getByLabel(/Starting balance/).fill('125000');
  await button(page, 'Start Tracking').click();
  await page.getByRole('region', { name: 'Net balance' }).getByText('IDR 125.000', { exact: true }).waitFor();
  await tabs.waitFor();
  await add.waitFor();
  await noOverflow('/dashboard');
  assert.match(await tabs.evaluate(el => getComputedStyle(el).backdropFilter), /blur\(20px\)/);
  assert.ok(await tabs.evaluate(el => parseFloat(getComputedStyle(el).borderTopLeftRadius) > 50));

  await page.setViewportSize({ width: 768, height });
  await tabs.waitFor({ state: 'hidden' });
  await add.waitFor({ state: 'hidden' });
  await page.getByRole('heading', { name: 'Dashboard', exact: true }).waitFor();
  await button(page, 'Search Ctrl/⌘ K').waitFor();
  await page.keyboard.press('Control+k');
  await glassPanel(page.getByRole('dialog', { name: 'Command palette' }), true);
  await page.keyboard.press('Escape');
  await page.setViewportSize({ width, height });

  await button(page, 'Switch ledger').click();
  const ledgers = page.getByRole('dialog', { name: 'Switch ledger' });
  await ledgers.getByText('Personal activity').waitFor();
  await glassPanel(ledgers, true);
  await button(ledgers, 'Close dialog').click();
  await button(page, 'Hide amounts').click();
  const mask = page.getByRole('region', { name: 'Net balance' }).getByText('••••', { exact: true }).first();
  await mask.waitFor();
  assert.equal(await page.getByRole('region', { name: 'Net balance' }).getByText('IDR 125.000', { exact: true }).count(), 0);
  await mask.evaluate(el => Promise.all(el.getAnimations().map(animation => animation.finished)));
  assert.equal(await mask.evaluate(el => getComputedStyle(el).filter), 'blur(4px)');
  await button(page, 'Show amounts').click();

  await add.click();
  await wizard.waitFor();
  await tabs.waitFor({ state: 'hidden' });
  const sheetBounds = await wizard.boundingBox();
  assert.deepEqual(sheetBounds, { x: 0, y: 0, width, height });
  await noOverflow('/transactions/new');
  await button(wizard, 'Calculator').click();
  const multiply = button(wizard, 'Multiply');
  const key = await multiply.boundingBox();
  assert.ok(key && key.height >= 52 && key.width >= 44 && key.y + key.height <= height, 'calculator key must fit the viewport and meet the touch target');
  await button(wizard, '1').click();
  await multiply.click();
  await button(wizard, '2').click();
  await button(wizard, 'Equals').click();
  const amount = page.getByLabel('Amount (IDR)');
  assert.equal(await amount.inputValue(), '2');
  assert.ok(await amount.evaluate(node => parseFloat(getComputedStyle(node).fontSize)) >= 16);
  await button(page, 'Continue').click();
  await wizard.getByRole('button', { name: /Mobile Cash/ }).waitFor();
  await button(page, 'Quick Save').waitFor();
  await button(page, 'Add details →').click();
  await page.getByLabel('Description (optional)').fill('Mobile groceries');
  const notes = page.getByLabel('Notes (optional)');
  await notes.fill('Verified at ' + width + 'px');
  await page.setViewportSize({ width, height: 450 });
  const save = button(page, 'Save Transaction');
  const footer = await save.boundingBox();
  assert.ok(footer && footer.y + footer.height <= 450, 'save stays visible when the form viewport shrinks');
  await noOverflow('/transactions/new details');
  await save.click();
  await page.setViewportSize({ width, height });
  await page.getByRole('heading', { name: 'Mobile groceries', exact: true }).waitFor();
  await tabs.waitFor({ state: 'hidden' });
  await page.getByRole('link', { name: 'Back', exact: true }).click();
  await page.getByRole('region', { name: 'Today' }).getByRole('link', { name: /Mobile groceries/ }).waitFor();
  await noOverflow('/transactions');
  await page.evaluate(() => scrollTo(0, document.documentElement.scrollHeight));
  const lastRow = page.getByRole('region', { name: 'Today' }).getByRole('listitem').last();
  assert.ok((await lastRow.boundingBox()).y + (await lastRow.boundingBox()).height <= (await tabs.boundingBox()).y,
    'The bottom bar overlaps the last activity row');
  await button(page, 'Income').click();
  await page.getByRole('link', { name: /Mobile groceries/ }).waitFor({ state: 'hidden' });
  await page.getByRole('link', { name: /Opening balance/ }).waitFor();
  await button(page, 'All').click();
  await page.getByRole('link', { name: /Mobile groceries/ }).waitFor();
  await button(page, 'Filters').click();
  const sheet = page.getByRole('dialog', { name: 'Activity filters' });
  await glassPanel(sheet, true);
  assert.equal(await sheet.getAttribute('data-slot'), 'drawer-content');
  await sheet.getByRole('searchbox', { name: 'Search', exact: true }).fill('Mobile');
  await noOverflow('/transactions filters');
  await button(sheet, 'Apply filters').click();
  await sheet.waitFor({ state: 'hidden' });
  await page.getByRole('link', { name: /Opening balance/ }).waitFor({ state: 'hidden' });
  await page.getByRole('link', { name: /Mobile groceries/ }).waitFor();

  await button(tabs, 'More').click();
  await page.getByRole('dialog', { name: 'More' }).getByRole('link', { name: 'Accounts', exact: true }).click();
  await page.getByRole('link', { name: /Mobile Cash/ }).waitFor();
  await noOverflow('/accounts');
  await button(tabs, 'More').click();
  await page.getByRole('dialog', { name: 'More' }).getByRole('link', { name: 'Settings', exact: true }).click();
  await storage(page).waitFor();
  await noOverflow('/settings');
  const activityName = page.getByLabel('Activity name');
  await activityName.fill('Updated mobile activity');
  await button(page, 'Save activity settings').click();
  await tab('Home').click();
  await page.getByRole('region', { name: 'Net balance' }).waitFor();
  assert.equal(await page.getByText('Another task is running. Try again in a moment.').count(), 0);
  await button(tabs, 'More').click();
  await page.getByRole('dialog', { name: 'More' }).getByRole('link', { name: 'Settings', exact: true }).click();
  await page.waitForFunction(() =>
    [...document.querySelectorAll('input')].some(input => input.value === 'Updated mobile activity'));
  for (const name of ['Budgets', 'Goals', 'Recurring', 'Assets']) await page.getByRole('checkbox', { name, exact: true }).check();
  await button(page, 'Save app preferences').click();
  await page.getByRole('status').filter({ hasText: 'App preferences saved.' }).waitFor();
  await tab('Plan').click();
  const plan = page.getByRole('navigation', { name: 'Plan sections' });
  await plan.waitFor();
  await page.getByRole('heading', { name: 'Budgets', exact: true }).waitFor();
  await noOverflow('/budgets');
  for (const name of ['Goals', 'Recurring', 'Assets']) await plan.getByRole('link', { name, exact: true }).waitFor();
  await plan.getByRole('link', { name: 'Loans' }).click();
  await page.getByRole('heading', { name: 'Debts & Loans' }).waitFor();
  await tab('Home').click();
  await page.getByRole('region', { name: 'Recent activity' }).getByRole('link', { name: /Mobile groceries/ }).waitFor();
  assert.deepEqual(external, []);
  await glassCsp(page);
  await page.context().close();
  console.log(`PASS ${width}×${height} ${flavor} floating glass, scroll clearance, mobile navigation, sheets, calculator touch targets, transaction save and date grouping, and no horizontal overflow on six routes`);
}

async function verifyWorkspace(width, height, flavor = 'latte') {
  const { page, external } = await glassDevice(width, height, flavor);
  async function noOverflow(path) {
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `${width}×${height} ${path} overflows`);
  }
  await noOverflow('/');
  assert.equal(await page.getByLabel('Primary currency').inputValue(), 'IDR');
  await button(page, 'Continue').click();
  await page.getByRole('radio', { name: /^Cash/ }).check();
  await page.getByLabel('Account name').fill('Pocket cash');
  await button(page, 'Continue').click();
  await page.getByLabel(/Starting balance/).fill('90000');
  await button(page, 'Start Tracking').click();
  await page.getByRole('region', { name: 'Net balance' }).getByText('IDR 90.000', { exact: true }).waitFor();
  await noOverflow('/dashboard');

  const sidebar = page.getByRole('complementary').first();
  const bounds = await sidebar.boundingBox();
  const main = await page.getByRole('main').boundingBox();
  assert.equal(bounds.width, width<1024?64:248);
  assert.equal(bounds.x, 0, 'the sidebar sits at the left edge');
  assert.equal(main.x, bounds.width + 9, 'the workspace has an 8px inset and 1px frame beside the sidebar');
  assert.equal(main.x + main.width + 9, await page.evaluate(() => document.documentElement.clientWidth),
    'the workspace has the same inset at the right edge');
  await button(page, 'Open activity').click();
  const menu = page.locator('[data-slot="dropdown-menu-content"]');
  await glassPanel(menu, true);
  await page.keyboard.press('Escape');
  await menu.waitFor({ state: 'hidden' });
  if(width<1024){
    await nav(page, 'Accounts').focus();
    await page.locator('[data-slot="tooltip-content"]').filter({ hasText: 'Accounts' }).waitFor();
  }
  await page.keyboard.press(process.platform==='darwin'?'Meta+k':'Control+k');
  const palette = page.getByRole('dialog', { name: 'Command palette' });
  await glassPanel(palette, true);
  await palette.getByRole('combobox', { name: 'Search commands and this activity' }).fill('accounts');
  await palette.getByRole('option', { name: /Accounts/ }).first().waitFor();
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await palette.waitFor({ state:'hidden' });
  await page.getByRole('heading', { name: 'Accounts', exact: true }).waitFor();
  await noOverflow('/accounts');

  await nav(page, 'Transactions').click();
  await page.getByRole('table').waitFor();
  await noOverflow('/transactions');
  await button(page, 'Filters').click();
  const filters = page.locator('[data-slot="popover-content"]');
  await glassPanel(filters, true);
  await filters.getByLabel('Date basis').waitFor();
  await button(filters, 'Apply filters').click();
  await filters.waitFor({ state: 'hidden' });
  await page.getByRole('checkbox', { name: /^Select Opening balance/ }).check();
  const bulk = page.getByRole('group', { name: 'Bulk transaction actions' });
  await glassPanel(bulk, true);
  assert.equal(await bulk.evaluate(el => getComputedStyle(el).position), 'fixed');
  if (width === 768 && flavor === 'latte') {
    const cdp = await page.context().newCDPSession(page);
    await cdp.send('Emulation.setEmulatedMedia', {
      features: [{ name: 'prefers-reduced-transparency', value: 'reduce' }]
    });
    assert.equal(await bulk.evaluate(el => getComputedStyle(el).backdropFilter), 'none');
    assert.equal(await bulk.evaluate(el => getComputedStyle(el).backgroundColor), 'rgb(230, 233, 239)');
    await cdp.send('Emulation.setEmulatedMedia', { features: [] });
    await cdp.detach();
  }
  await button(bulk, 'Clear selection').click();
  await bulk.waitFor({ state: 'hidden' });
  await page.getByRole('link', { name:'New transaction', exact:true }).click();
  await button(page, 'Calculator').click();
  await button(page, '1').click();
  await button(page, 'Multiply').click();
  await button(page, '2').click();
  await button(page, 'Equals').click();
  assert.equal(await page.getByLabel('Amount (IDR)').inputValue(), '2');
  await button(page, 'Continue').click();
  await button(page, 'Save Transaction').click();
  await page.getByRole('heading', { name: 'Expense', exact: true }).waitFor();

  await nav(page, 'Settings').click();
  await storage(page).waitFor();
  await noOverflow('/settings');
  await button(storage(page), 'Switch Mode').click();
  const dialog = page.getByRole('dialog', { name: 'Switch to Cloud Sync' });
  await glassPanel(dialog, true);
  assert.equal(await dialog.getAttribute('data-slot'), 'dialog-content');
  await button(dialog, 'Close dialog').click();
  await dialog.waitFor({ state:'hidden' });
  await page.getByRole('checkbox', { name:'Budgets', exact:true }).check();
  await button(page, 'Save app preferences').click();
  await nav(page, 'Budgets').click();
  await page.getByRole('heading', { name:'Budgets', exact:true }).waitFor();
  await noOverflow('/budgets');
  assert.deepEqual(external, []);
  await glassCsp(page);
  await page.context().close();
  console.log(`PASS ${width}×${height} ${flavor} floating bulk pill, glass panels, workspace rail, command keyboard, Dialog, filters, IDR Cash setup, calculator, and no horizontal overflow on six routes`);
}

async function verifyCategoryIcons(width, height) {
  const mobile = width < 768;
  const { page } = await newDevice(false, { viewport: { width, height }, hasTouch: mobile });
  const name = 'Groceries';
  const phoneNav = page.getByRole('navigation', { name: 'Mobile navigation' });
  const noOverflow = async () => assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  async function open(name) {
    if (!mobile) return nav(page, name).click();
    const tab = { Transactions: 'Activity', Dashboard: 'Home', Budgets: 'Plan' }[name];
    if (tab) return phoneNav.getByRole('link', { name: tab, exact: true }).click();
    await button(phoneNav, 'More').click();
    await page.getByRole('dialog', { name: 'More' }).getByRole('link', { name, exact: true }).click();
  }
  async function pick(dialog, label, id) {
    const parentId = await dialog.getAttribute('id');
    await dialog.getByRole('button', { name: /^Category icon/ }).click();
    const picker = page.getByRole('dialog', { name: 'Choose category icon' });
    await glassPanel(picker, true);
    assert.equal(await picker.evaluate((_, id) => getComputedStyle(document.getElementById(id)).backdropFilter, parentId), 'none');
    assert.equal(await picker.getAttribute('data-slot'), mobile ? 'drawer-content' : 'popover-content');
    await picker.getByRole('searchbox', { name: 'Search icons' }).fill(label);
    await picker.locator(`[data-icon-choice="${id}"]`).getByText(label, { exact: true }).click();
    await picker.waitFor({ state: 'hidden' });
  }

  await button(page, 'Continue').click();
  await page.getByRole('radio', { name: /^Cash/ }).check();
  await page.getByLabel('Account name').fill('Cash Wallet');
  await button(page, 'Continue').click();
  await button(page, 'Start Tracking').click();
  await page.getByRole('region', { name: 'Net balance' }).waitFor();
  const activeNavigation = mobile ? phoneNav : page.getByRole('navigation', { name: 'Main navigation' });
  await activeNavigation.locator('[aria-current="page"] [data-icon-weight="fill"]').waitFor();

  await open('Categories');
  await button(page, 'New category').click();
  let edit = page.getByRole('dialog', { name: 'New category' });
  await edit.getByLabel('Name', { exact: true }).fill(name);
  await edit.getByRole('button', { name: /Automatic · Groceries/ }).locator('[data-category-icon="shopping-cart"]').waitFor();
  await pick(edit, 'Coffee & snacks', 'coffee');
  await button(edit, 'Save category').click();
  const cell = page.getByRole('cell', { name, exact: true });
  await cell.locator('[data-category-icon="coffee"]').waitFor();
  await noOverflow();

  const categoryActions = page.getByRole('row').filter({ has: cell }).locator('[data-row-actions]');
  assert.deepEqual(await categoryActions.getByRole('button').allTextContents(), ['', '']);
  assert.equal(await categoryActions.getByRole('button', { name: /More actions/ }).count(), 0);
  await button(categoryActions, 'Delete').click();
  const categoryDelete = page.getByRole(mobile ? 'dialog' : 'alertdialog', { name: 'Delete Groceries?' });
  await categoryDelete.waitFor();
  await button(categoryDelete, 'Close dialog').click();
  await categoryDelete.waitFor({ state:'hidden' });
  await button(categoryActions, 'Edit').click();
  edit = page.getByRole('dialog', { name: 'Edit category' });
  await pick(edit, 'Shopping', 'shopping-bag');
  await button(edit, 'Save category').click();
  await cell.locator('[data-category-icon="shopping-bag"]').waitFor();
  await page.reload();
  await button(page, 'Open Personal activity').click();
  await open('Categories');
  await cell.locator('[data-category-icon="shopping-bag"]').waitFor();

  await open('Transactions');
  await page.getByRole('link', { name: 'New transaction', exact: true }).click();
  await page.getByLabel('Amount (IDR)').fill('12000');
  await button(page, 'Continue').click();
  const category = button(page, name);
  await category.locator('[data-category-icon="shopping-bag"]').waitFor();
  await noOverflow();
  await category.click();
  await button(page, mobile ? 'Quick Save' : 'Save Transaction').click();
  await page.getByRole('heading', { name, exact: true }).waitFor();
  await page.locator('main [data-category-icon="shopping-bag"]').waitFor();
  if (mobile) await page.getByRole('link', { name: 'Back', exact: true }).click();
  else await open('Transactions');
  const txRow = page.getByRole(mobile ? 'listitem' : 'row').filter({ has: page.getByRole('link', { name: /^Groceries/ }) });
  await txRow.locator('[data-category-icon="shopping-bag"]').waitFor();
  const actions = txRow.getByRole('group', { name: 'Actions for Groceries' });
  assert.deepEqual(await actions.getByRole('button').allTextContents(), ['', '', '']);
  for (const label of ['Edit Groceries', 'Delete Groceries', 'More actions Groceries']) {
    const bounds = await button(actions, label).boundingBox();
    assert.ok(bounds && bounds.width >= (mobile ? 44 : 32) && bounds.height >= (mobile ? 44 : 32), label);
  }
  const beforeOverflow = page.url();
  const more = button(actions, 'More actions Groceries');
  await more.click();
  const overflow = mobile ? page.getByRole('dialog', { name:'Groceries', exact:true }) : page.getByRole('menu');
  await glassPanel(overflow, true);
  assert.equal(await overflow.getAttribute('data-slot'), mobile ? 'drawer-content' : 'dropdown-menu-content');
  const view = overflow.getByRole(mobile ? 'button' : 'menuitem', { name:'View details', exact:true });
  await view.waitFor();
  if (mobile) assert.ok((await view.boundingBox()).height >= 48);
  assert.equal(page.url(), beforeOverflow);
  await noOverflow();
  if (mobile) await button(overflow, 'Close dialog').click();
  else await page.keyboard.press('Escape');
  await overflow.waitFor({ state:'hidden' });
  assert.equal(page.url(), beforeOverflow);
  await button(actions, 'Delete Groceries').click();
  const transactionDelete = page.getByRole(mobile ? 'dialog' : 'alertdialog', { name:'Delete transaction?' });
  await transactionDelete.waitFor();
  assert.equal(page.url(), beforeOverflow);
  await button(transactionDelete, 'Close dialog').click();
  await transactionDelete.waitFor({ state:'hidden' });
  if (mobile) await more.click();
  else {
    await more.focus();
    await page.keyboard.press('Enter');
  }
  await view.click();
  await page.getByRole('heading', { name, exact:true }).waitFor();
  assert.notEqual(page.url(), beforeOverflow);
  if (mobile) await page.getByRole('link', { name: 'Back', exact: true }).click();

  await open('Settings');
  await page.getByRole('checkbox', { name: 'Budgets', exact: true }).check();
  await button(page, 'Save app preferences').click();
  await page.getByText('App preferences saved.').waitFor();
  await open('Budgets');
  await button(page, 'Set limit').click();
  const budget = page.getByRole('dialog', { name: 'Set budget limit' });
  await budget.getByRole('combobox', { name: /^Category/ }).selectOption({ label: name });
  await budget.getByLabel('Limit', { exact: true }).fill('50000');
  await button(budget, 'Save').click();
  await page.getByRole('heading', { name, exact: true }).locator('[data-category-icon="shopping-bag"]').waitFor();
  await noOverflow();

  await open('Dashboard');
  await page.getByRole('region', { name: 'Recent activity' }).locator('[data-category-icon="shopping-bag"]').filter({ visible: true }).waitFor();
  await page.getByRole('region', { name: 'Your plan' }).locator('[data-category-icon="shopping-bag"]').waitFor();
  await noOverflow();
  await page.context().close();
  console.log(`PASS ${width}×${height} icon-only row actions, adaptive glass overflow, delete confirmations, touch targets, detail navigation, category icons, and no horizontal overflow`);
}


async function verifyThemes(width, height) {
  const mobile = width < 768;
  const { page } = await newDevice(false, { viewport: { width, height }, hasTouch: mobile, colorScheme: 'light' });
  const context = page.context();
  const violations = [];
  await context.exposeFunction('reportThemeCsp', event => violations.push(event));
  await context.addInitScript(() => {
    document.addEventListener('securitypolicyviolation', event => {
      window.reportThemeCsp(event.violatedDirective + ': ' + event.blockedURI);
    });
  });
  const bases = { latte: '#eff1f5', frappe: '#303446', macchiato: '#24273a', mocha: '#1e1e2e' };
  const options = [
    ['latte', 'Catppuccin Latte'], ['frappe', 'Catppuccin Frappé'], ['macchiato', 'Catppuccin Macchiato'],
    ['mocha', 'Catppuccin Mocha'], ['system', 'System Auto']
  ];
  const appearance = page.getByRole('region', { name: 'Appearance' });
  async function verifyRoot(target, preference, flavor) {
    const actual = await target.evaluate(() => {
      const root = document.documentElement;
      return [
        root.dataset.theme, root.classList.contains('dark'),
        document.querySelector('meta[name="theme-color"]').content,
        getComputedStyle(root).backgroundColor,
        localStorage.getItem('fgps:theme')
      ];
    });
    const rgb = bases[flavor].slice(1).match(/../g).map(hex => parseInt(hex, 16)).join(', ');
    assert.deepEqual(actual, [flavor, flavor !== 'latte', bases[flavor], 'rgb(' + rgb + ')', preference]);
  }
  async function saved(preference) {
    await page.waitForFunction(expected => new Promise((resolve, reject) => {
      const open = indexedDB.open('financial-gps', 1);
      open.onerror = () => reject(open.error);
      open.onsuccess = () => {
        const db = open.result;
        const read = db.transaction('records').objectStore('records').get('app.preferences');
        read.onsuccess = () => { db.close(); resolve(read.result?.theme === expected); };
        read.onerror = () => { db.close(); reject(read.error); };
      };
    }), preference);
  }
  async function verifyContrast() {
    const ratios = await appearance.locator('[data-theme]').evaluateAll(previews => {
      // A one-pixel canvas resolves CSS color-mix and rgb into sRGB for WCAG luminance.
      const canvas = document.createElement('canvas');
      canvas.width = canvas.height = 1;
      const pixel = canvas.getContext('2d');
      const luminance = ([r, g, b]) => [r, g, b].reduce((sum, channel, index) => {
        const s = channel / 255;
        return sum + [0.2126, 0.7152, 0.0722][index] * (s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4);
      }, 0);
      return previews.flatMap(preview => {
        const tokens = getComputedStyle(preview);
        const pairs = [
          ['foreground', 'background'], ['card-foreground', 'card'], ['muted-foreground', 'card'],
          ['muted-foreground', 'secondary'], ['accent-foreground', 'accent'],
          ['primary-foreground', 'primary'], ['primary-foreground', 'primary-hover'],
          ['primary-ink', 'secondary'], ['money-in', 'money-in-soft'], ['money-out', 'money-out-soft'],
          ['warning', 'secondary'], ['pending', 'secondary'], ['info', 'secondary'], ['transfer', 'secondary']
        ];
        function color(name, on) {
          pixel.clearRect(0, 0, 1, 1);
          if (on) {
            pixel.fillStyle = tokens.getPropertyValue('--' + on).trim();
            pixel.fillRect(0, 0, 1, 1);
          }
          const sample = document.createElement('span');
          sample.style.color = 'var(--' + name + ')';
          preview.appendChild(sample);
          pixel.fillStyle = getComputedStyle(sample).color;
          sample.remove();
          pixel.fillRect(0, 0, 1, 1);
          return luminance(pixel.getImageData(0, 0, 1, 1).data);
        }
        return pairs.map(([fg, bg]) => {
          const text = color(fg), fill = color(bg, 'background');
          return [preview.dataset.theme + ' ' + fg + '/' + bg, (Math.max(text, fill) + 0.05) / (Math.min(text, fill) + 0.05)];
        });
      });
    });
    assert.deepEqual(ratios.filter(([, ratio]) => ratio < 4.5)
      .map(([name, ratio]) => name + ' contrast ' + ratio.toFixed(2)), []);
  }
  await button(page, 'Continue').click();
  await page.getByRole('radio', { name: /^Cash/ }).check();
  await page.getByLabel('Account name').fill('Theme test cash');
  await button(page, 'Continue').click();
  await button(page, 'Start Tracking').click();
  await page.getByRole('region', { name: 'Net balance' }).waitFor();
  async function openAppearance() {
    if (mobile) {
      await button(page.getByRole('navigation', { name: 'Mobile navigation' }), 'More').click();
      await page.getByRole('dialog', { name: 'More' }).getByRole('link', { name: 'Settings', exact: true }).click();
    } else {
      await nav(page, 'Settings').click();
    }
    await appearance.waitFor();
  }
  await openAppearance();
  assert.equal(await appearance.locator('[data-swatch]').count(), 48);
  for (const [flavor, name] of options.slice(0, 4)) {
    const preview = button(appearance, name).locator('[data-theme="' + flavor + '"]');
    assert.equal(await preview.evaluate(node => getComputedStyle(node).getPropertyValue('--ctp-base').trim()), bases[flavor]);
  }
  const system = button(appearance, 'System Auto');
  assert.equal(await system.locator('[data-theme]').count(), 2);
  assert.equal(await system.getAttribute('aria-pressed'), 'true');
  await verifyContrast();
  const early = await context.newPage();
  await early.route('**/*', route => route.request().resourceType() === 'script' ? route.abort() : route.continue());
  const documentToken = await page.evaluate(() => window.themeTestToken = crypto.randomUUID());
  for (const [preference, name] of options) {
    await button(appearance, name).click();
    const flavor = preference === 'system' ? 'latte' : preference;
    await verifyRoot(page, preference, flavor);
    assert.equal(await button(appearance, name).getAttribute('aria-pressed'), 'true');
    assert.equal(await page.evaluate(() => window.themeTestToken), documentToken);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), preference + ' overflows');
    await saved(preference);
    await early.goto(origin + '/settings');
    await verifyRoot(early, preference, flavor);
  }
  await early.close();
  await page.emulateMedia({ colorScheme: 'dark' });
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'mocha');
  await verifyRoot(page, 'system', 'mocha');
  await page.emulateMedia({ colorScheme: 'light' });
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'latte');
  await verifyRoot(page, 'system', 'latte');

  const palette = page.getByRole('dialog', { name: 'Command palette' });
  async function searchPalette(query) {
    if (mobile) await button(page, 'Search').click();
    else await page.keyboard.press('Control+k');
    await palette.getByRole('combobox', { name: 'Search commands and this activity' }).fill(query);
    await palette.getByRole('option', { name: new RegExp(query, 'i') }).click();
  }
  await searchPalette('Theme previews');
  const quick = page.getByRole('dialog', { name: 'Theme previews' });
  await button(quick, 'Catppuccin Frappé').click();
  await verifyRoot(page, 'frappe', 'frappe');
  await button(quick, 'Close dialog').click();
  await searchPalette('Theme: Catppuccin Macchiato');
  await verifyRoot(page, 'macchiato', 'macchiato');
  await saved('macchiato');
  await page.emulateMedia({ colorScheme: 'dark' });
  await verifyRoot(page, 'macchiato', 'macchiato');
  await page.evaluate(() => localStorage.removeItem('fgps:theme'));
  await page.reload();
  await button(page, 'Open Personal activity').waitFor();
  await verifyRoot(page, 'macchiato', 'macchiato');
  await button(page, 'Open Personal activity').click();
  await page.getByRole('region', { name: 'Net balance' }).waitFor();
  await openAppearance();
  assert.equal(await button(appearance, 'Catppuccin Macchiato').getAttribute('aria-pressed'), 'true');
  assert.deepEqual(violations, [], 'theme switching or boot violated CSP');
  await context.close();
  console.log(`PASS ${width}×${height} all five themes, scoped previews, WCAG AA token contrast, live OS changes, quick actions, persisted reload, first-paint boot, CSP, and no overflow`);
}

try {
  const template = readFileSync('src/app.html', 'utf8');
  const boot = template.match(/<script id="theme-boot">([\s\S]*?)<\/script>/)[1];
  const hash = 'sha256-' + createHash('sha256').update(boot).digest('base64');
  assert.ok(readFileSync('vite.config.ts', 'utf8').includes(hash), 'Web CSP must authorize the theme boot script');
  const tauri = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));
  assert.ok(tauri.app.security.csp.includes("'" + hash + "'"), 'Native CSP must authorize the theme boot script');
  let available = false;
  for (let i = 0; i < 100 && !available; i++) {
    assert.equal(server.exitCode, null, 'Vite failed to start');
    try { available = (await fetch(origin)).ok; } catch {}
    if (!available) await new Promise(resolve => setTimeout(resolve, 100));
  }
  assert.ok(available, 'Standalone Vite ' + serverMode + ' server did not respond');
  browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH });
  await verifyThemes(390, 844);
  await verifyThemes(768, 1024);
  await verifyThemes(1280, 800);
  const { page: primary } = await newDevice(false);
  const second = await primary.context().newPage();
  await second.goto(origin);
  const contention = second.getByRole('alert');
  await contention.getByText(/already open in another tab/).waitFor();
  await primary.close();
  await button(second, 'Try again').click();
  await second.getByRole('heading', { name: 'Your activity' }).waitFor();
  await second.context().close();
  console.log('PASS same-origin OPFS lock message and recovery after closing the other tab');

  await verifyWizards();
  for (const flavor of ['latte', 'mocha']) {
    await verifyMobile(390, 844, flavor);
    await verifyMobile(360, 800, flavor);
    await verifyWorkspace(768, 1024, flavor);
    await verifyWorkspace(1280, 800, flavor);
  }
  await verifyWorkspace(1920, 1080);
  await verifyCategoryIcons(390, 844);
  await verifyCategoryIcons(768, 1024);
  await verifyCategoryIcons(1280, 800);
  const { page: a, external } = await newDevice();
  const local = a.getByRole('region', { name: 'Create activity' });
  assert.ok(await local.getByRole('radio', { name: 'Local Only' }).isChecked());
  await local.getByLabel('Name', { exact: true }).fill('Travel Ledger');
  await local.getByLabel('Display currency').selectOption('USD');
  await button(local, 'Create activity').click();
  await createAccount(a);
  await createTransaction(a, 'Café initial posting');
  await a.getByRole('link', { name: 'Your activity' }).click();
  await a.waitForURL(origin + '/');
  await a.reload();
  await button(a, 'Open Travel Ledger').click();
  await nav(a, 'Accounts').click();
  await a.getByRole('heading', { name: 'Travel Wallet' }).waitFor();
  await search(a, 'cafe', 'Café initial posting');
  await settings(a);
  await storage(a).getByText('Local Only', { exact: true }).waitFor();
  assert.deepEqual(external, []);
  assert.equal(counts.requests, 0);
  console.log('PASS standalone Local Only creation, account and transaction entry, OPFS reload, accent-normalized search, and zero outbound storage requests');

  await button(storage(a), 'Switch Mode').click();
  const enroll = a.getByRole('dialog', { name: 'Switch to Cloud Sync' });
  await credentials(enroll, 'invalid-token');
  await button(enroll, 'Test Connection').click();
  await expectText(enroll.getByRole('alert'), authError);
  const prior = counts.probes;
  await credentials(enroll, token);
  await button(enroll, 'Test Connection').click();
  await enroll.getByText('Connection verified.').waitFor();
  assert.equal(counts.probes, prior + 1);
  await button(enroll, 'Confirm Cloud Sync').click();
  await storage(a).getByText('Turso Cloud Sync').waitFor();
  await expectText(labelValue(a, 'Turso endpoint'), 'ph•••.turso.io');
  assert.ok(!(await storage(a).innerText()).includes(host));
  await verifyTokenAbsent(a);
  const savedProbe = counts.probes;
  await button(storage(a), 'Test Connection').click();
  await storage(a).getByText('Connection verified.').waitFor();
  assert.equal(counts.probes, savedProbe + 1);
  await sync(a);
  await storage(a).locator('time[datetime]').waitFor();
  await expectText(labelValue(a, 'Pending outbox changes'), /^0$/);
  console.log('PASS Settings live SELECT 1, typed authentication rejection, confirmed enrollment, masked endpoint, no token in the DOM or Web Storage, and manual sync');

  const { page: b } = await newDevice();
  const cloud = b.getByRole('region', { name: 'Create activity' });
  await cloud.getByLabel('Name', { exact: true }).fill('Travel Ledger');
  await cloud.getByLabel('Display currency').selectOption('USD');
  await cloud.getByRole('radio', { name: 'Cloud Sync' }).check();
  await credentials(cloud, 'invalid-token');
  await button(cloud, 'Create activity').click();
  await expectText(cloud.getByRole('alert'), authError);
  assert.equal(await button(b, 'Open Travel Ledger').count(), 0);
  await credentials(cloud, token);
  await button(cloud, 'Create activity').click();
  await settings(b);
  await storage(b).getByText('Turso Cloud Sync').waitFor();
  await sync(b);
  await search(b, 'cafe', 'Café initial posting');
  await verifyTokenAbsent(b);
  console.log('PASS Cloud Sync onboarding validates before creation and a second browser profile pulls the ledger');

  async function conflict(localText, remoteText) {
    counts.networkDisabled = true;
    await createTransaction(a, remoteText);
    await createTransaction(b, localText);
    await settings(a);
    await settings(b);
    await expectText(labelValue(b, 'Pending outbox changes'), /^[1-9]\d*$/);
    counts.networkDisabled = false;
    await sync(a);
    await sync(b);
    await button(storage(b), 'Review sync conflict').click();
    const dialog = b.getByRole('dialog', { name: 'Resolve sync conflict' });
    await dialog.getByText('Local edit version').waitFor();
    await dialog.getByText('Remote generation', { exact: true }).waitFor();
    return dialog;
  }

  let dialog = await conflict('Overridden offline entry', 'Marché remote entry');
  await dialog.getByLabel('Safety backup passphrase').fill('fixture-backup-passphrase');
  const download = b.waitForEvent('download');
  await button(dialog, 'Export encrypted safety backup').click();
  const archive = readFileSync(await (await download).path());
  assert.equal(archive.subarray(0, 8).toString(), 'FINGPSBK');
  assert.ok(!archive.includes('Overridden offline entry'));
  assert.ok(await button(dialog, 'Accept Remote Version').isDisabled());
  await dialog.getByRole('checkbox').check();
  await button(dialog, 'Accept Remote Version').click();
  await dialog.waitFor({ state: 'hidden' });
  await expectText(labelValue(b, 'Pending outbox changes'), /^0$/);
  await search(b, 'marche', 'Marché remote entry');
  assert.equal(await b.getByText('Overridden offline entry', { exact: true }).count(), 0);
  console.log('PASS two-device conflict metadata, encrypted safety backup download, and Accept Remote with rebuilt FTS search');

  dialog = await conflict('Résumé kept entry', 'Losing remote entry');
  await button(dialog, 'Keep Local Version').click();
  await dialog.waitFor({ state: 'hidden' });
  await sync(a);
  await search(a, 'resume', 'Résumé kept entry');
  console.log('PASS Keep Local publishes a new generation visible on the other device');

  await a.context().close();
  tokens.delete(token);
  await settings(b);
  await button(storage(b), 'Update credentials').click();
  const rotate = b.getByRole('dialog', { name: 'Update Turso credentials' });
  assert.equal(await rotate.getByLabel('Turso Auth Token').inputValue(), '');
  await rotate.getByLabel('Turso Auth Token').fill(rotated);
  await button(rotate, 'Save new token').click();
  await rotate.waitFor({ state: 'hidden' });
  await verifyTokenAbsent(b);
  const rotatedProbe = counts.probes;
  await button(storage(b), 'Test Connection').click();
  await storage(b).getByText('Connection verified.').waitFor();
  assert.equal(counts.probes, rotatedProbe + 1);
  await button(storage(b), 'Switch Mode').click();
  await button(b.getByRole('dialog', { name: 'Switch to Local Only' }), 'Confirm Local Only').click();
  await storage(b).getByText('Local Only', { exact: true }).waitFor();
  await search(b, 'resume', 'Résumé kept entry');
  await verifyTokenAbsent(b);
  console.log('PASS credential rotation starts with an empty password field and confirmed Local Only switch preserves ledger rows');

  const outboundBeforeRestore = counts.requests;
  await b.getByRole('link', { name: 'Your activity' }).click();
  await button(b, 'Restore from backup').click();
  const restore = b.getByRole('dialog', { name: 'Restore encrypted backup' });
  const chooser = b.waitForEvent('filechooser');
  await button(restore, 'Choose backup…').click();
  await (await chooser).setFiles({
    name: 'safety.fgpsbackup', mimeType: 'application/octet-stream', buffer: archive
  });
  await restore.getByLabel('Archive passphrase').fill('fixture-backup-passphrase');
  await button(restore, 'Verify backup').click();
  await restore.getByLabel('New activity name').fill('Recovered Travel');
  await restore.getByLabel('New activity passphrase or PIN').fill('fixture-ledger-credential');
  await restore.getByLabel('Repeat new credential').fill('fixture-ledger-credential');
  await button(restore, 'Restore as new activity').click();
  await search(b, 'overridden', 'Overridden offline entry');
  await button(b, 'Open activity').click();
  await b.getByRole('menuitem', { name: 'Travel Ledger', exact: true }).click();
  await b.getByRole('heading', { name: 'Dashboard', exact: true }).waitFor();
  await search(b, 'resume', 'Résumé kept entry');
  await button(b, 'Open activity').click();
  await b.getByRole('menuitem', { name: 'Recovered Travel', exact: true }).click();
  await b.getByRole('heading', { name: 'Unlock activity' }).waitFor();
  await b.getByLabel('Passphrase or PIN').fill('fixture-ledger-credential');
  await button(b, 'Unlock').click();
  await b.getByRole('heading', { name: 'Dashboard', exact: true }).waitFor();
  await search(b, 'overridden', 'Overridden offline entry');
  await button(b, 'Lock').click();
  await button(b, 'Open Recovered Travel').click();
  await b.getByLabel('Passphrase or PIN').fill('fixture-ledger-credential');
  await button(b, 'Unlock').click();
  await b.getByRole('heading', { name: 'Dashboard', exact: true }).waitFor();
  await search(b, 'overridden', 'Overridden offline entry');
  assert.equal(counts.requests, outboundBeforeRestore);
  console.log('PASS offline encrypted restore, ledger switching, and passphrase lock/unlock from the launcher');
} catch (error) {
  if (lastPage) console.error(await lastPage.locator('main').innerText({ timeout: 1_000 }).catch(() => ''));
  throw error;
} finally {
  await browser?.close();
  server.kill();
  close();
  rmSync(temporary, { recursive: true, force: true });
}
