import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { chromium } from 'playwright';

const origin = 'http://127.0.0.1:1431';
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', 'dev', '--host', '127.0.0.1', '--port', '1431', '--strictPort'], {
  stdio: ['ignore', 'ignore', 'inherit']
});
const external = [];
let browser;
let page;
const button = (root, name) => root.getByRole('button', { name, exact: true });
const navigate = name => page.getByRole('navigation', { name: 'Main navigation' }).getByRole('link', { name, exact: true }).click();

try {
  for (let attempt = 0; attempt <= 100; attempt++) {
    if (await fetch(origin).then(response => response.ok).catch(() => false)) break;
    if (server.exitCode !== null || attempt === 100) throw new Error('Vite did not start.');
    await delay(200);
  }
  browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH });
  page = await browser.newPage({ viewport: { width: 1400, height: 880 } });
  page.setDefaultTimeout(20000);
  page.on('request', request => {
    if (request.url().startsWith('http') && new URL(request.url()).origin !== origin) external.push(request.url());
  });
  await page.goto(origin);
  assert.equal(await page.evaluate(() => typeof window.__TAURI_INTERNALS__), 'undefined');
  await button(page, 'Continue').click();
  await page.getByLabel('Account name').fill('Planning wallet');
  await button(page, 'Continue').click();
  await page.getByLabel('Starting balance (IDR)').fill('10000');
  await page.getByLabel('Set up a payday cycle').check();
  await page.getByLabel('Payday', { exact: true }).fill('1');
  await button(page, 'Start Tracking').click();
  await page.waitForURL('**/dashboard');
  await page.getByRole('link', { name: 'View cash plan' }).waitFor();
  console.log('PASS optional onboarding creates a payday cycle on the standalone web app');

  await navigate('Settings');
  const calendar = page.getByRole('region', { name: 'Payday calendar' });
  await calendar.getByText(/Current cycle/).waitFor();
  await calendar.getByLabel('Payday', { exact: true }).fill('15');
  await button(calendar, 'Save calendar settings').click();
  await calendar.getByText(/Change scheduled for/).waitFor();
  await button(calendar, 'Cancel scheduled change').click();
  await calendar.getByText(/Change scheduled for/).waitFor({ state: 'hidden' });
  await calendar.getByText(/Current cycle/).waitFor();

  await page.getByRole('checkbox', { name: 'Budgets', exact: true }).check();
  await button(page, 'Save app preferences').click();
  await navigate('Categories');
  await button(page, 'New category').click();
  const category = page.getByRole('dialog', { name: 'New category' });
  await category.getByLabel('Name', { exact: true }).fill('Utilities');
  await button(category, 'Save category').click();
  await page.getByText('Utilities', { exact: true }).waitFor();

  await navigate('Budgets');
  await button(page, 'Add budget').click();
  const budget = page.getByRole('dialog', { name: 'Add cycle budget' });
  await budget.getByLabel('Budget name').fill('Household utilities');
  await budget.getByLabel('Limit in IDR').fill('32000');
  await budget.getByRole('checkbox', { name: 'Utilities' }).check();
  await button(budget, 'Save budget').click();
  await page.getByRole('heading', { name: 'Household utilities' }).waitFor();
  assert.equal(await button(page, 'Payday cycle').getAttribute('aria-pressed'), 'true');
  await button(page, 'Monthly').click();
  await button(page, 'Set limit').waitFor();
  console.log('PASS scheduled payday changes, cycle budget creation, and legacy monthly budget access');

  await navigate('Cash plan');
  const forecast = page.getByRole('heading', { name: 'Cash forecast' });
  await forecast.waitFor();
  await button(page, 'Add to cash plan').click();
  let edit = page.getByRole('dialog', { name: 'Add to cash plan' });
  await edit.getByLabel('Name', { exact: true }).fill('Internet service');
  await edit.getByLabel('Amount in IDR').fill('15000');
  await button(edit, 'Save entry').click();
  await page.getByRole('heading', { name: 'Internet service' }).waitFor();
  await page.getByText(/At-due gap/).waitFor();

  await button(page, 'Add to cash plan').click();
  edit = page.getByRole('dialog', { name: 'Add to cash plan' });
  await edit.getByLabel('Name', { exact: true }).fill('Freelance work');
  await edit.getByRole('combobox', { name: /^Type/ }).selectOption('uncertainIncome');
  await edit.getByLabel('Amount in IDR').fill('6000');
  await button(edit, 'Save entry').click();
  await page.getByLabel('Include in scenario').check();
  await page.getByText('With selected income', { exact: true }).first().waitFor();

  await page.reload();
  // Browser reload discards the unlocked session. OPFS retains the ledger and its plan.
  await page.waitForLoadState('networkidle');
  if (new URL(page.url()).pathname === '/') {
    await page.getByRole('button', { name: /^Open Personal activity$/i }).click();
    await page.waitForURL('**/dashboard');
    await navigate('Cash plan');
  }
  await forecast.waitFor();
  await page.getByRole('heading', { name: 'Internet service' }).waitFor();
  await page.getByRole('heading', { name: 'Freelance work' }).waitFor();
  assert.deepEqual(external, []);
  console.log('PASS cash gap, uncertain-income scenario, OPFS restart, and zero external requests');
} catch (error) {
  if (page) console.error(await page.locator('main').innerText().catch(() => ''));
  throw error;
} finally {
  await browser?.close();
  server.kill();
}
