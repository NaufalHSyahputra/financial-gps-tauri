# Debugging runbook

[Documentation home](README.md) · [Features](FEATURES.md) · [Build and release](BUILD_AND_RELEASE.md)

Reproduce errors with a disposable ledger. Capture the runtime, app version, action, and the visible message or correlation ID. Use redacted diagnostics when sharing output. Keep Turso tokens, passphrases, and ledger contents out of logs.

## Shared core and preflight

1. From the repository root, check the installed versions and run the relevant gates.

   ```sh
   node --version
   rustc --version
   wasm-bindgen --version
   cargo test --workspace --all-targets
   cargo clippy --workspace --all-targets -- -D warnings
   cargo check --locked -p core-engine --target wasm32-unknown-unknown
   npm run check
   npm run audit:drivers
   ```

2. A failure in both runtimes often starts in the shared Rust operation or schema. For a failure in one runtime, follow the operation from the route through its driver and inspect the serialized inputs and typed error.
3. If you changed a native command or its types, regenerate both sets of TypeScript definitions.

   ```sh
   cargo run -p financial-gps --features binding-generator --bin generate-bindings
   node scripts/generate-driver-contract.mjs
   npm run audit:drivers
   npm run check
   ```

| Common error | Fix |
| --- | --- |
| Missing WASM target or mismatched generated bindings | Install the Rust target and exact `wasm-bindgen-cli` version in the [build guide](BUILD_AND_RELEASE.md#prerequisites-and-toolchain), then run `npm run wasm:build`. |
| Driver audit finds a direct Tauri import in a route or view | Call the shared driver. Keep Tauri bindings and transport under the native driver. |
| Stale session or revision | Reopen the intended ledger. Refresh the data and repeat the preview before applying a change. |

The standard checks print to the terminal. The [Phase 4 browser test](../tests/storage-ui.mjs), [worker test](../tests/web-driver.mjs), [native IPC tests](../src-tauri/ipc/tests.rs), and [driver audit](../scripts/audit-frontend-drivers.mjs) are starting points for a reproducible case.

## Standalone Web

1. Run `npm run dev` and open `http://localhost:1420` in a regular browser. Check `typeof window.__TAURI_INTERNALS__` in the browser console. The standalone value is `"undefined"`.
2. In Chrome DevTools, use **Sources** and select the database worker in the thread list. In Firefox, open the Debugger and use its worker or thread list. Stop in [db.worker.ts](../src/lib/drivers/web/db.worker.ts) to inspect operations. Use **Network** to inspect worker script, WASM, and expected optional Turso requests without copying auth headers.
3. Read isolation and storage state from the page console.

   ```js
   console.log({ secure: isSecureContext, isolated: crossOriginIsolated })
   console.log(await navigator.storage.estimate())
   const opfs = await navigator.storage.getDirectory()
   for await (const entry of opfs.values()) console.log(entry.kind, entry.name)
   ```

   The SQLite access-handle pool uses `.financial-gps-sqlite`. The app uses `financial-gps` for additional OPFS data such as encrypted images, attachments, and backups. Pool filenames are internal. Use an encrypted backup to export a recoverable copy. [OPFS adapter](../src/lib/drivers/web/opfs.ts) · [OPFS API](https://developer.mozilla.org/en-US/docs/Web/API/File_System_API/Origin_private_file_system)

4. In Chrome **Application → IndexedDB**, or Firefox **Storage → Indexed DB**, inspect `financial-gps → records`. After enrolling with a disposable fixture token, check `vault:key:v1` for an AES-GCM key with `extractable: false`. Encrypted entries use the `vault:secret:` prefix and store a nonce and ciphertext. In the browser storage panel, check Local Storage and Session Storage for the fixture token. [Web vault](../src/lib/drivers/web/secret-vault.ts)
5. If a disposable profile needs a full reset, export any ledger you need first. Close the app's other tabs, then clear site data for this exact origin through browser settings. This removes its ledgers, vault, preferences, and cached app. For a stale service worker alone, unregister the service worker and reload online before resorting to a full reset.

```sh
npx playwright install chromium
npm run test:web-driver
npm run test:storage-ui
```

The tests start local Vite servers and intercept Turso requests with a SQLite-backed fixture. The first exercises the worker and vault directly. The second drives the real UI through account setup, the transaction wizard, enrollment, sync conflicts, and restore. To test the built app's offline cache, run `npm run build` then `node tests/web-offline.mjs`. [Worker test](../tests/web-driver.mjs) · [Browser UI test](../tests/storage-ui.mjs) · [Offline test](../tests/web-offline.mjs)

| Common error | Fix |
| --- | --- |
| OPFS unavailable or browser storage operation failed | Check for HTTPS or localhost, verify storage quota, and try a fresh regular browser profile. Stay on the original scheme, hostname, and port to reopen existing data. |
| `crossOriginIsolated` is false or the test reports missing `SharedArrayBuffer` | Inspect the actual document and worker response headers in Network. Apply COOP and COEP to both. Confirm worker and WASM URLs return their expected files and MIME types. |
| CSP blocks a worker, WASM module, or connection | Compare the response and page policies with the [hosting guide](BUILD_AND_RELEASE.md#web-production-hosting). Check the browser console for the directive that failed. Keep assets same-origin and send Turso traffic only to a validated Turso endpoint. |
| Sync is offline or the token is rejected | Use **Settings → Storage & Sync → Test Connection**. Verify the URL and rotate an expired or incorrect token. For an unresolved version conflict, review the two versions in the app. |

To check calculation or formatting issues, run `npm run test:calculator` and use **Settings → Activity settings** to confirm the **Primary currency**. Test an amount with Calculator off, then test it with Calculator on. Separately verify the account’s currency if the activity contains older accounts in other currencies. Statement import and exchange-rate commands return `feature_unavailable`.

## Tauri Desktop

1. Run the development app from a terminal. Use the terminal for build output, panic output, and Rust backtraces. Debug builds expose the WebView inspector through the context menu or **Ctrl+Shift+I** on Linux/Windows and **Cmd+Option+I** on macOS where the WebView supports it. Use the inspector for JavaScript, IPC results, and rendering. [Tauri debugging](https://v2.tauri.app/develop/debug/)
2. Run with backtraces when investigating a panic.

   ```sh
   RUST_BACKTRACE=1 npm run tauri -- dev
   ```

   In PowerShell, set `$env:RUST_BACKTRACE = '1'` before running `npm run tauri -- dev`. `RUST_LOG` requires a logging subscriber, which this checkout lacks. For a Rust service error, use terminal panic output, the typed IPC error, or a Rust debugger. [Tauri startup](../src-tauri/ipc/mod.rs) · [Native dependencies](../src-tauri/Cargo.toml)

3. Find Tauri's app data directory. These are typical paths, subject to platform environment and installation conventions.

   | OS | Typical app data directory |
   | --- | --- |
   | Linux | `${XDG_DATA_HOME:-$HOME/.local/share}/app.financialgps.desktop` |
   | macOS | `~/Library/Application Support/app.financialgps.desktop` |
   | Windows | `%APPDATA%\app.financialgps.desktop` |

   Look for `ledger-registry.json` and `ledgers/`. Portable paths are recorded in the registry. For native diagnostics, close a disposable plain test ledger cleanly and inspect a copy with the SQLite CLI in read-only mode. Use Financial GPS to unlock SQLCipher ledgers and verify encrypted backups. [Tauri path resolver](https://docs.rs/tauri/2.12.7/tauri/path/struct.PathResolver.html#method.app_data_dir) · [App identifier](../src-tauri/tauri.conf.json) · [Registry](../src-tauri/storage/registry.rs)

   ```sh
   read -r -p 'Path to the plain test database copy: ' fgps_test_db
   sqlite3 -readonly "$fgps_test_db" 'PRAGMA quick_check;'
   ```

4. If quick unlock, scheduled backup, or Turso enrollment fails at the vault, start with the matching platform service. Linux uses the D-Bus Secret Service backend. KWallet can serve it when configured to expose that interface. On macOS check the active Keychain and its prompts. On Windows check Credential Manager under the same user account as the app. Retry with a disposable secret in an unlocked desktop session.

   ```sh
   cargo test -p financial-gps --lib native_secret_vault_round_trips_and_revokes_distinct_secret_slots -- --ignored
   ```

   This opt-in test uses real vault entries and revokes its test data. [Native vault and test](../src-tauri/crypto/native_secret_vault.rs)

5. On Linux, compare the executable's library dependencies with the host before diagnosing startup as a ledger problem.

   ```sh
   ldd target/release/financial-gps
   readelf -d target/release/financial-gps | grep NEEDED
   ```

| Common error | Fix |
| --- | --- |
| Missing `libcrypto.so.4` or another runtime library | Check `ldd`. The previous package was linked on CachyOS and used `libcrypto.so.4`. Rebuild on the oldest supported base distribution with matching libraries. Avoid symlinking incompatible OpenSSL major versions. [Release evidence](V1_RELEASE_SIGN_OFF.md) |
| Secret Service or keychain unavailable | Launch inside an unlocked user session with a running credential provider. Use the ledger passphrase or PIN while resolving quick-unlock issues. |
| Registry is busy | Close other instances using that app data directory. Reopen after they exit cleanly. |
| Integrity check failed | Preserve the files. Use a verified backup and restore as a new ledger to validate recovery. |

## Android

1. Install the [Android toolchain](BUILD_AND_RELEASE.md#android-toolchain), start an emulator or connect a debug-enabled phone, and run from the repository root.

   ```sh
   adb devices
   npm run tauri -- android init
   npm run tauri -- android dev
   ```

   Run initialization once per fresh checkout that needs Android. The generated project is local to that checkout.

2. Use Android Studio **Logcat** or filter by process after launch. For an early crash, use the crash buffer. If the generated debug project appends an application ID suffix, use that full ID for `pidof`.

   ```sh
   adb logcat --pid="$(adb shell pidof -s app.financialgps.desktop | tr -d '\r')"
   adb logcat -b crash
   ```

3. Open `chrome://inspect/#devices` in desktop Chrome, choose the running debug WebView, and inspect its console and network requests. Inspect the **generated debug** manifest and network policy if Android blocks access to the local Vite server. Keep HTTPS for Turso in release builds. [Tauri mobile debugging](https://v2.tauri.app/develop/#developing-your-mobile-application) · [Android network security](https://developer.android.com/privacy-and-security/security-config)

| Common error | Fix |
| --- | --- |
| Device is missing or unauthorized | Enable developer options and USB debugging, accept the authorization prompt, or start an emulator. |
| WebView cannot load the dev server | Check reachability to the host on port 1420 and confirm Tauri supplied the reachable host to Vite. Keep any cleartext allowance scoped to debugging. |
| Secure vault error | Test with the device unlocked and inspect Logcat for errors from the Android credential store. Try a clean debug install with a disposable ledger before changing signing or package identity. |
| NDK or OpenSSL link failure | Check the ABI and NDK used by the generated project. The app uses bundled SQLCipher. Any OpenSSL inputs supplied to a cross-build must match the target ABI. |

## iOS

1. On macOS, install the [iOS toolchain](BUILD_AND_RELEASE.md#ios-toolchain) and choose a simulator or connected device.

   ```sh
   npm run tauri -- ios init
   npm run tauri -- ios dev
   ```

   For a device that needs a reachable development host, use `npm run tauri -- ios dev --host`. For Xcode debugging, use `npm run tauri -- ios dev --open` and leave the terminal process running.

2. Enable Web Inspector on the device. In Safari on the Mac, enable developer features and choose the device and app in **Develop**. Use Xcode's debug console for Rust/native output. For a connected device, select it in **Window → Devices and Simulators** to reach device diagnostics. [Apple Web Inspector](https://developer.apple.com/documentation/safari-developer-tools/inspect-apps-and-devices) · [Tauri mobile development](https://v2.tauri.app/develop/#developing-your-mobile-application)
3. If the vault fails, inspect the signed app's bundle ID, team, provisioning profile, and Keychain entitlements in Xcode. Compare simulator and device behavior with a disposable ledger. Financial GPS uses the protected Apple store on iOS. [Native vault](../src-tauri/crypto/native_secret_vault.rs) · [Tauri iOS signing](https://v2.tauri.app/distribute/sign/ios/)

| Common error | Fix |
| --- | --- |
| Tauri CLI has no `ios` command | Use the same checkout and locked npm dependencies on a Mac with Xcode. |
| No provisioning profile matches the app | Select the correct Apple team and bundle identifier, then regenerate or select a matching profile. |
| Safari cannot attach | Enable Web Inspector, trust the connected Mac, and use a development build. Verify the app is open. |
| Device has no access to Vite | Use the reachable development host selected by Tauri. Check network reachability and device trust settings. |

Mobile troubleshooting follows the repository's native entry point and Tauri's platform workflows. The Phase 4 gate exercised browser scenarios and native Rust/IPC tests. Android and iOS device runs require separate verification.
