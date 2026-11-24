---
name: mobile-webview-review
description: Autonomous mobile and Tauri WebView UI/UX self-review using Playwright viewports, DOM geometry audits, and visual screenshot inspection.
---

# Mobile & WebView Self-Review Protocol

Apply this skill alongside [Unslop](../unslop/SKILL.md) and [Web Design Guidelines](../web-design-guidelines/SKILL.md) whenever reviewing, debugging, or polishing mobile or Tauri WebView UI/UX.

## Workflow
1. **Start Local Server:** Ensure the Vite dev server is running (`npm run dev -- --host 127.0.0.1 --port 1432`) and reuse the existing Playwright/Chromium runtime (`process.env.CHROMIUM_PATH` if set).
2. **Sweep Mobile Viewports:** Inspect target routes across both mobile WebView sizes (`isMobile: true`, `hasTouch: true`):
   - `360x800` (compact phone)
   - `412x915` (standard modern phone)
3. **Run Automated DOM Geometry Audit:** Run `node .agents/skills/mobile-webview-review/scripts/mobile-audit.mjs <url> --out /tmp/mobile-review` to capture screenshots and detect:
   - Horizontal overflow (`document.documentElement.scrollWidth > window.innerWidth`)
   - Undersized touch targets (`width < 44` or `height < 44` on visible buttons, links, inputs, and selects, excluding inline prose links and `.chart-bar`)
   - Mobile input auto-zoom risks (`fontSize < 16px` on `input`, `select`, `textarea`)
   - Header/action button collisions (`getBoundingClientRect()` overlap)
4. **Inspect Screenshots & Fix:** Read the generated PNG screenshots in `/tmp/mobile-review/`, combine visual findings with the JSON geometry report, fix all layout/UX defects, and re-run the audit until zero violations remain.
