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

async function visit(path) {
  // Navigate within the SPA to keep the active browser ledger open.
  await page.evaluate(href => {
    const link = document.createElement('a');
    link.href = href;
    document.body.append(link);
    link.click();
    link.remove();
  }, path);
  await page.waitForURL(url => url.pathname === path);
}

async function noOverflow(path) {
  const { scrollWidth, width } = await page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth, width: innerWidth
  }));
  assert.ok(scrollWidth <= width, `${path} overflowed at ${width}px (${scrollWidth}px scroll width)`);
}

async function box(locator) {
  await locator.waitFor();
  const bounds = await locator.boundingBox();
  assert.ok(bounds);
  return bounds;
}

async function addExpense(description, daysAgo) {
  await navigate('Transactions');
  await page.getByRole('link', { name: 'New transaction', exact: true }).click();
  await page.getByLabel('Amount (IDR)').fill('1500');
  await button(page, 'Continue').click();
  await button(page, 'Add details').click();
  const date = page.getByLabel('Date', { exact: true });
  const today = await date.inputValue();
  await date.fill(new Date(Date.parse(today) - daysAgo * 86400000).toISOString().slice(0, 10));
  await page.getByLabel('Description (optional)').fill(description);
  await button(page, 'Save Transaction').click();
  await page.getByRole('heading', { name: description, exact: true }).waitFor();
  return { path: new URL(page.url()).pathname, today };
}

async function addGroupHistory() {
  await navigate('Shared groups');
  await button(page, 'New group').click();
  let modal = page.getByRole('dialog', { name: 'New shared group' });
  await modal.getByLabel('Name', { exact: true }).fill('Weekend trip');
  await modal.getByLabel('Your name in this group').fill('Me');
  await button(modal, 'Save group').click();
  await page.getByRole('heading', { name: 'Weekend trip', exact: true }).waitFor();
  await button(page, 'Add member').click();
  modal = page.getByRole('dialog', { name: 'Group member' });
  await modal.getByLabel('Name', { exact: true }).fill('Alex');
  await button(modal, 'Save').click();
  await page.getByRole('button', { name: 'Edit Alex', exact: true }).waitFor();
  await button(page, 'Split expense').click();
  modal = page.getByRole('dialog', { name: 'Split expense' });
  await modal.getByLabel('Description', { exact: true }).fill('Train tickets');
  await modal.getByLabel('Amount', { exact: true }).fill('25000');
  await button(modal, 'Save').click();
  await button(page, 'Allocation for Train tickets').waitFor();

  await button(page, 'Pay back').click();
  modal = page.getByRole('dialog', { name: 'Pay back' });
  await modal.getByRole('combobox', { name: /^From/ }).selectOption({ label: 'Alex' });
  await modal.getByRole('combobox', { name: /^To/ }).selectOption({ label: 'Me' });
  await modal.getByLabel('Amount', { exact: true }).fill('5000');
  await button(modal, 'Save').click();
  await modal.waitFor({ state: 'hidden' });
  return new URL(page.url()).pathname;
}

async function addAssetHistory() {
  await navigate('Assets');
  await page.getByRole('link', { name: 'New asset', exact: true }).click();
  await page.getByLabel('Name', { exact: true }).fill('Bicycle');
  await button(page, 'Save asset').click();
  await page.getByRole('heading', { name: 'Bicycle', exact: true }).waitFor();
  await button(page, 'Add valuation').click();
  const modal = page.getByRole('dialog', { name: 'Manual valuation' });
  await modal.getByLabel('Market value').fill('1200000');
  await button(modal, 'Save').click();
  await page.getByRole('cell', { name: 'Manual valuation' }).waitFor();
  return new URL(page.url()).pathname;
}

async function verifyMobile(width, height, transaction, { accountPath, groupPath, assetPath }) {
  await page.setViewportSize({ width, height });

  await visit('/dashboard');
  const activity = page.getByRole('region', { name: 'Recent activity' });
  const seeAll = await box(activity.getByRole('link', { name: 'See all', exact: true }));
  assert.ok(seeAll.width >= 44 && seeAll.height >= 44);
  await activity.getByRole('link', { name: 'See all', exact: true }).click();
  const title = page.getByRole('link', { name: /^Lunch at the market/ }).getByText('Lunch at the market', { exact: true });
  await title.waitFor();
  assert.ok(await title.evaluate(node => node.scrollWidth <= node.clientWidth), 'Transaction title fits without clipping');
  await noOverflow('/transactions');

  await visit('/budgets');
  const budgetPeriods = page.getByRole('group', { name: 'Budget period' });
  assert.equal(await budgetPeriods.evaluate(node => getComputedStyle(node).position), 'sticky');
  await button(budgetPeriods, 'Monthly').click();
  await button(page, 'Set limit').waitFor();
  const [history, limit] = await Promise.all([box(button(page, 'Historical suggestions')), box(button(page, 'Set limit'))]);
  assert.equal(history.y, limit.y);
  await noOverflow('/budgets');

  await visit('/planning');
  await page.getByRole('heading', { name: 'Internet service' }).waitFor();
  await noOverflow('/planning');

  await visit('/reports/spending');
  const start = new Date(Date.parse(transaction.today) - 7 * 86400000).toISOString().slice(0, 10);
  await page.getByLabel('From', { exact: true }).fill(start);
  await page.getByRole('combobox', { name: /^Group by/ }).selectOption('day');
  const chart = page.locator('[data-chart]');
  await chart.locator('button[data-mark]').nth(3).waitFor();
  const widths = await chart.locator('button[data-mark]').evaluateAll(bars => bars.map(bar => bar.offsetWidth));
  assert.ok(widths.every(markWidth => markWidth > 0 && markWidth < 44), `Chart bar widths: ${widths.join(', ')}`);
  const bars = chart.locator('button[data-mark]');
  await bars.first().focus();
  await bars.first().press('ArrowRight');
  assert.ok(await bars.nth(1).evaluate(node => node === document.activeElement));
  const expenses = page.getByRole('list', { name: 'Report periods' }).getByRole('link', { name: 'View expense transactions' });
  assert.match(await expenses.first().getAttribute('href'), /basis=reporting.*pnl=1.*direction=debit/);
  const expense = await box(expenses.first());
  assert.ok(expense.width >= 44 && expense.height >= 44);
  await noOverflow('/reports/spending');

  await visit('/rules');
  const more = await box(page.getByRole('button', { name: 'More actions Groceries at the neighborhood market' }));
  assert.ok(more.x >= 0 && more.x + more.width <= width && more.width >= 44 && more.height >= 44);
  await button(page, 'New rule').click();
  const modal = page.getByRole('dialog', { name: 'New rule' });
  const field = modal.getByRole('combobox', { name: /^Field/ }).first();
  await field.selectOption('signedAmount');
  await modal.getByRole('combobox', { name: /^Currency/ }).waitFor();
  await modal.evaluate(async node => {
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    await Promise.all(node.getAnimations().map(animation => animation.finished.catch(() => {})));
  });
  const [remove, fieldBounds, operator, value] = await Promise.all([
    box(button(modal, 'Remove condition')), box(field), box(modal.getByRole('combobox', { name: /^Operator/ })),
    box(modal.getByRole('textbox', { name: 'Value', exact: true }))
  ]);
  assert.ok(remove.width >= 44 && remove.height >= 44);
  assert.ok(remove.x >= fieldBounds.x + fieldBounds.width && Math.abs(remove.y - fieldBounds.y) <= 1);
  assert.ok(operator.y > remove.y && value.y > operator.y);
  await noOverflow('/rules');
  await button(modal, 'Close dialog').click();

  await visit('/settings');
  const settingsSections = page.getByRole('navigation', { name: 'Settings sections' });
  for (const [name, anchor] of [
    ['Appearance', 'settings-appearance'], ['Calendar & Sync', 'settings-calendar-sync'],
    ['Ledger & App', 'settings-ledger-app'], ['Security', 'settings-security']
  ]) {
    await settingsSections.getByRole('link', { name, exact: true }).click();
    await page.waitForURL(url => url.hash === '#' + anchor);
    const target = await box(page.locator('#' + anchor));
    const navigation = await box(settingsSections);
    assert.ok(target.y + 1 >= navigation.y + navigation.height, name + ' scrolls clear of the sticky links');
  }
  await noOverflow('/settings');

  await visit('/transactions/new');
  const close = await box(page.getByRole('link', { name: 'Close new transaction' }));
  assert.ok(close.x > width / 2 && close.width >= 44 && close.height >= 44);
  await noOverflow('/transactions/new');

  await visit(transaction.path);
  const sections = page.getByRole('group', { name: 'Transaction sections' });
  const description = page.getByRole('textbox', { name: 'Description', exact: true });
  await description.waitFor();
  await button(sections, 'Splits & Files').click();
  await page.getByRole('heading', { name: 'Attachments' }).waitFor();
  await description.waitFor({ state: 'hidden' });
  await button(sections, 'Corrections').click();
  await button(page, 'Void transaction').waitFor();
  await page.getByRole('heading', { name: 'Attachments' }).waitFor({ state: 'hidden' });
  await button(sections, 'Details').click();
  await description.fill('');
  await button(sections, 'Splits & Files').click();
  await button(page, 'Save transaction').click();
  await description.waitFor();
  assert.equal(await button(sections, 'All').getAttribute('aria-pressed'), 'true');
  await description.fill('Lunch at the market');
  await noOverflow('/transactions/[id]');

  await visit(accountPath);
  const accountSections = page.getByRole('group', { name: 'Account sections' });
  await button(accountSections, 'History').click();
  await page.getByText('No balance history yet.', { exact: true }).waitFor();
  await page.getByRole('heading', { name: 'Account settings' }).waitFor({ state: 'hidden' });
  await button(accountSections, 'Statements / Loan').click();
  await page.getByText('Statement cycles and loan schedules are available on credit card and loan accounts.').waitFor();
  await button(accountSections, 'Details').click();
  await page.getByRole('heading', { name: 'Account settings' }).waitFor();
  await noOverflow('/accounts/[id]');

  await visit('/accounts/new');
  await button(page, 'Continue').click();
  const [back, next] = await Promise.all([box(button(page, 'Back')), box(button(page, 'Continue'))]);
  assert.ok(back.height >= 44 && next.height >= 44 && back.y === next.y && back.width === next.width);
  await noOverflow('/accounts/new');

  await visit(groupPath);
  const expenseCards = page.getByRole('list', { name: 'Expenses', exact: true });
  await button(expenseCards, 'Allocation for Train tickets').click();
  await page.locator('[data-slot="popover-content"]').getByText('Alex', { exact: true }).waitFor();
  await page.keyboard.press('Escape');
  await page.getByRole('list', { name: 'Paybacks', exact: true }).getByText('Alex → Me').waitFor();
  await noOverflow('/groups/[id]');

  await visit(assetPath);
  const historyCards = page.getByRole('list', { name: 'Asset history' });
  await historyCards.getByText('Manual valuation', { exact: true }).waitFor();
  await historyCards.getByRole('button', { name: /^Edit valuation on/ }).click();
  await button(page.getByRole('dialog', { name: 'Manual valuation' }), 'Close dialog').click();
  await noOverflow('/assets/[id]');
  console.log(`PASS ${width}×${height} mobile planning, spending chart and drill-down, rules, Settings anchors, editors, and group and asset cards with zero horizontal overflow`);
}

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
  await page.getByRole('checkbox', { name: 'Shared groups', exact: true }).check();
  await page.getByRole('checkbox', { name: 'Assets', exact: true }).check();
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

  await navigate('Accounts');
  const accountPath = await page.getByRole('link', { name: 'Planning wallet', exact: true }).getAttribute('href');
  assert.match(accountPath, /^\/accounts\//);
  await addExpense('Coffee on the way', 3);
  const transaction = await addExpense('Lunch at the market', 1);
  const groupPath = await addGroupHistory();
  const assetPath = await addAssetHistory();
  await navigate('Rules');
  await button(page, 'New rule').click();
  const rule = page.getByRole('dialog', { name: 'New rule' });
  await rule.getByLabel('Rule name').fill('Groceries at the neighborhood market');
  await rule.getByLabel('Value', { exact: true }).fill('market');
  await rule.getByLabel('Text', { exact: true }).fill('Neighborhood market');
  await button(rule, 'Save rule').click();
  await rule.waitFor({ state: 'hidden' });
  const routes = { accountPath, groupPath, assetPath };
  await verifyMobile(360, 800, transaction, routes);
  await verifyMobile(412, 915, transaction, routes);
  assert.deepEqual(external, []);
} catch (error) {
  if (page) console.error(await page.locator('main').innerText().catch(() => ''));
  throw error;
} finally {
  await browser?.close();
  server.kill();
}
