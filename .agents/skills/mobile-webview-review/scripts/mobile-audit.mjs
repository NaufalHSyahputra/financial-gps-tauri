import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { parseArgs } from 'node:util';
import { chromium } from 'playwright';

const viewports = [{ width: 360, height: 800 }, { width: 412, height: 915 }];

function audit(expectedWidth) {
  const rect = element => element.getBoundingClientRect();
  const visible = element => {
    if (element.closest('[inert], [aria-hidden="true"]') ||
        !element.checkVisibility({ opacityProperty: true, visibilityProperty: true })) return false;
    const { clip, clipPath } = getComputedStyle(element);
    if (clip === 'rect(0px, 0px, 0px, 0px)' || clipPath === 'inset(50%)') return false;
    const bounds = rect(element);
    return bounds.width > 0 && bounds.height > 0;
  };
  const identify = element => ({
    tag: element.tagName.toLowerCase(),
    id: element.id || undefined,
    label: element.getAttribute('aria-label') || undefined,
    text: element.matches('input, select, textarea') ? undefined : element.textContent.trim().replace(/\s+/g, ' ').slice(0, 80)
  });
  // A modal's backdrop blocks the page and any lower drawer.
  const modal = [...document.querySelectorAll('[role="dialog"], [role="alertdialog"]')]
    .filter(element => element.matches('[aria-modal="true"], [data-slot="drawer-content"]') && visible(element)).at(-1);
  const elements = selector => [...(modal ?? document).querySelectorAll(selector)].filter(visible);
  const inlineProseLink = element => element.matches('a[href]') &&
    !element.matches('[role="button"], [data-slot="button"]') &&
    getComputedStyle(element).display === 'inline' &&
    [...(element.closest('p, li')?.childNodes ?? [])].some(node => node.nodeType === Node.TEXT_NODE && node.textContent.trim());
  const actions = 'button, a[href], [role="button"]';
  const fields = 'input:not([type="hidden"]), select, textarea';
  const controls = elements(actions + ', ' + fields)
    .filter(element => !element.matches('.chart-bar') && !inlineProseLink(element));
  const meetsTouchSize = ({ width, height }) => width >= 44 && height >= 44;
  const undersizedTargets = controls.flatMap(element => {
    const bounds = rect(element);
    // A visible associated label can provide the touch area for a native control.
    if (meetsTouchSize(bounds) || [...(element.labels ?? [])].some(label => visible(label) && meetsTouchSize(rect(label)))) return [];
    return [{ element: identify(element), width: bounds.width, height: bounds.height }];
  });
  const smallInputFonts = elements(fields).flatMap(element => {
    const fontSize = parseFloat(getComputedStyle(element).fontSize);
    return fontSize < 16 ? [{ element: identify(element), fontSize }] : [];
  });

  const headers = 'header, [role="banner"], [data-slot="drawer-header"], [data-slot="dialog-header"], [data-slot="sheet-header"], [data-slot="alert-dialog-header"]';
  function collisionBounds(element) {
    let { left, right, top, bottom } = rect(element);
    for (let parent = element.parentElement; parent; parent = parent.parentElement) {
      const { overflowX, overflowY } = getComputedStyle(parent);
      if (overflowX === 'visible' && overflowY === 'visible') continue;
      const clip = rect(parent);
      if (overflowX !== 'visible') {
        left = Math.max(left, clip.left);
        right = Math.min(right, clip.right);
      }
      if (overflowY !== 'visible') {
        top = Math.max(top, clip.top);
        bottom = Math.min(bottom, clip.bottom);
      }
    }
    return { left, right, top, bottom };
  }
  const actionBounds = controls.filter(element => element.matches(actions)).map(element => ({ element, bounds: collisionBounds(element) }));
  const titleBounds = elements(headers).flatMap(header =>
    [...header.querySelectorAll('h1, h2, h3, [role="heading"], p')]
      .filter(element => !element.closest(actions) && visible(element))
      .map(element => ({ element, bounds: collisionBounds(element) }))
  );
  const collisions = [];
  for (const [index, first] of actionBounds.entries()) {
    for (const second of [...actionBounds.slice(index + 1), ...titleBounds]) {
      if (first.element.contains(second.element) || second.element.contains(first.element)) continue;
      // The fixed mobile navigation intentionally overlays content during scroll.
      const navigation = '[aria-label="Mobile navigation"]';
      if (first.element.closest(navigation) !== second.element.closest(navigation)) continue;
      const width = Math.min(first.bounds.right, second.bounds.right) - Math.max(first.bounds.left, second.bounds.left);
      const height = Math.min(first.bounds.bottom, second.bounds.bottom) - Math.max(first.bounds.top, second.bounds.top);
      if (width > 1 && height > 1) {
        collisions.push({ action: identify(first.element), overlaps: identify(second.element), width, height });
      }
    }
  }

  const { scrollWidth } = document.documentElement;
  return {
    innerWidth,
    scrollWidth,
    horizontalOverflow: scrollWidth > innerWidth || scrollWidth > expectedWidth,
    undersizedTargets,
    smallInputFonts,
    collisions
  };
}

async function main() {
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: { out: { type: 'string' } }
  });
  if (positionals.length !== 1 || !values.out) {
    throw new Error('Usage: node mobile-audit.mjs <url> --out <directory>');
  }
  const url = new URL(positionals[0]);
  if (!['http:', 'https:'].includes(url.protocol)) throw new Error('Use an HTTP or HTTPS URL.');

  await mkdir(values.out, { recursive: true });
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH });
  const results = [];
  try {
    for (const viewport of viewports) {
      const screenshot = `mobile-${viewport.width}x${viewport.height}.png`;
      const page = await browser.newPage({ viewport, isMobile: true, hasTouch: true, reducedMotion: 'reduce' });
      try {
        const response = await page.goto(url.href);
        if (!response?.ok()) throw new Error(`Page returned HTTP ${response?.status() ?? 'unknown'}.`);
        await page.waitForFunction(() => {
          if (!document.getElementById('theme-boot')) return true;
          const main = document.querySelector('main#content');
          return main?.innerText.trim() && ![...main.querySelectorAll('[role="status"]')]
            .some(status => /^Loading (local activity|activity settings)/.test(status.textContent.trim()));
        }, null, { timeout: 15_000 });
        await page.screenshot({ path: join(values.out, screenshot), fullPage: true, animations: 'disabled' });
        const checks = await page.evaluate(audit, viewport.width);
        results.push({ viewport, pathname: new URL(page.url()).pathname, screenshot, ...checks });
      } finally {
        await page.close();
      }
    }
  } finally {
    await browser.close();
  }

  const violations = results.reduce((total, result) =>
    total + Number(result.horizontalOverflow) + result.undersizedTargets.length +
    result.smallInputFonts.length + result.collisions.length, 0);
  const output = join(values.out, 'mobile-audit.json');
  await writeFile(output, JSON.stringify({ violations, results }, null, 2) + '\n');
  console.log(`${violations} violations across ${results.length} viewports. Report: ${output}`);
  process.exitCode = violations ? 1 : 0;
}

main().catch(error => {
  console.error(error.message);
  process.exitCode = 2;
});
