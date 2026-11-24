import { commands, type Theme_Serialize as Theme } from '../drivers/runtime';
import { session } from './session.svelte';

export const themeFlavors = {
  latte: { name: 'Latte', base: '#eff1f5' },
  frappe: { name: 'Frappé', base: '#303446' },
  macchiato: { name: 'Macchiato', base: '#24273a' },
  mocha: { name: 'Mocha', base: '#1e1e2e' }
} as const;

export type ThemeFlavor = keyof typeof themeFlavors;

export const themeOptions = [
  { value: 'system', name: 'System Auto', mode: 'Follow device light/dark' },
  { value: 'latte', name: 'Catppuccin Latte', mode: 'Light' },
  { value: 'frappe', name: 'Catppuccin Frappé', mode: 'Dark' },
  { value: 'macchiato', name: 'Catppuccin Macchiato', mode: 'Dark' },
  { value: 'mocha', name: 'Catppuccin Mocha', mode: 'Dark' }
] as const satisfies readonly { value: Theme; name: string; mode: string }[];

const cacheKey = 'fgps:theme';

function cachedPreference(): Theme {
  try {
    const stored = localStorage.getItem(cacheKey);
    if (stored === 'light') return 'latte';
    if (stored === 'dark') return 'mocha';
    return themeOptions.find(option => option.value === stored)?.value ?? 'system';
  } catch {
    return 'system';
  }
}

class ThemeStore {
  preference = $state<Theme>('system');
  private systemDark = $state(false);
  resolvedFlavor = $derived(this.preference === 'system' ? this.systemDark ? 'mocha' : 'latte' : this.preference);
  isDark = $derived(this.resolvedFlavor !== 'latte');
  error = $state('');
  private saved: Theme | undefined;
  private writes = Promise.resolve();
  private selection = 0;

  start() {
    const scheme = window.matchMedia('(prefers-color-scheme: dark)');
    this.systemDark = scheme.matches;
    this.preference = this.saved ?? cachedPreference();
    this.apply();
    const update = (event: MediaQueryListEvent) => {
      this.systemDark = event.matches;
      if (this.preference === 'system') this.apply();
    };
    scheme.addEventListener('change', update);
    return () => scheme.removeEventListener('change', update);
  }

  hydrate(preference: Theme) {
    if (this.saved !== undefined) return;
    this.saved = preference;
    this.preference = preference;
    this.apply();
  }

  select(preference: Theme) {
    if (preference === this.preference) return;
    const selection = ++this.selection;
    this.preference = preference;
    this.error = '';
    this.apply();
    this.writes = this.writes.then(async () => {
      try {
        const saved = await session.call(commands.appPreferencesSet({ theme: preference }), false);
        this.saved = saved.theme;
      } catch {
        if (selection !== this.selection) return;
        this.preference = this.saved ?? 'system';
        this.apply();
        this.error = 'Couldn’t save the theme. Try again.';
      }
    });
  }

  private apply() {
    const root = document.documentElement;
    root.dataset.theme = this.resolvedFlavor;
    root.classList.toggle('dark', this.isDark);
    const base = themeFlavors[this.resolvedFlavor].base;
    root.style.backgroundColor = base;
    root.style.colorScheme = this.isDark ? 'dark' : 'light';
    document.querySelector<HTMLMetaElement>('meta[name="theme-color"]')!.content = base;
    try { localStorage.setItem(cacheKey, this.preference); } catch { /* The driver still saves preferences. */ }
  }
}

export const theme = new ThemeStore();
