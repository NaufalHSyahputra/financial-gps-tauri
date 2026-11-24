import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import MagicString from 'magic-string';
import { defineConfig, loadEnv } from 'vite';

// Hash of #theme-boot in src/app.html, also pinned in tauri.conf.json.
const bootHash = 'sha256-032ydnPoN+7VnibCSz9uFuE129qxFR48+2fSYTnAnOM=' as const;

const connect = ['self', 'https://*.turso.io', 'wss://*.turso.io'] as const;
const workerHeaders = {
  'Cross-Origin-Opener-Policy': 'same-origin',
  'Cross-Origin-Embedder-Policy': 'require-corp',
  'Content-Security-Policy': "connect-src 'self' https://*.turso.io wss://*.turso.io; object-src 'none'; worker-src 'self'"
};

export default defineConfig(({ mode }) => ({
  plugins: [
    {
      name: 'financial-gps-csp-styles',
      enforce: 'pre',
      transform(code, id) {
        const file = id.split('?')[0];
        // These dependency templates emit static style attributes, which the production CSP blocks.
        const attributes = file.endsWith('/@sveltejs/kit/src/runtime/components/root.svelte')
          ? [/style="position: absolute; left: 0; top: 0; clip: rect\(0 0 0 0\);[^"]*"/, 'class="sr-only"'] as const
          : /\/bits-ui\/dist\/bits\/command\/components\/command-(item|link-item|group-items)\.svelte$/.test(file)
            ? [/style="display: contents;"/, 'class="contents"'] as const
            : undefined;
        if (!attributes) return;
        const match = attributes[0].exec(code);
        if (!match) return;
        const source = new MagicString(code);
        source.overwrite(match.index, match.index + match[0].length, attributes[1]);
        return { code: source.toString(), map: source.generateMap({ hires: true }) };
      }
    },
    tailwindcss(),
    sveltekit({
      adapter: adapter({ fallback: 'index.html' }),
      serviceWorker: { register: false },
      csp: {
        directives: {
          'script-src': ['self', 'wasm-unsafe-eval', bootHash],
          'style-src': ['self'],
          'worker-src': ['self'],
          'connect-src': [...connect],
          'object-src': ['none'],
          'base-uri': ['self'],
          'img-src': ['self', 'blob:', 'data:']
        }
      }
    })
  ],
  // Routine builds omit the optional callback timing report. Compiler diagnostics stay enabled.
  build: { rolldownOptions: { checks: { bundlerTimings: false } } },
  worker: { format: 'es' },
  clearScreen: false,
  server: {
    host: loadEnv(mode, false, 'TAURI_DEV_HOST').TAURI_DEV_HOST || 'localhost',
    headers: workerHeaders,
    watch: { ignored: ['**/src-tauri/**', '**/target/**'] }
  },
  preview: { headers: workerHeaders }
}));
