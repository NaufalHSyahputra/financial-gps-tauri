# Financial GPS documentation

Financial GPS is a personal finance app that saves changes locally before any optional sync. An **activity file** is a separate set of accounts, transactions, plans, and settings. Use the same interface in a browser or a Tauri app. The shared finance engine applies the same money and reporting rules across runtimes.

| Guide | Start here if you want to |
| --- | --- |
| [Features](FEATURES.md) | Record transactions, plan spending, or manage storage and backups |
| [Debugging](DEBUGGING.md) | Diagnose issues on Web, Desktop, Android, or iOS |
| [Build and release](BUILD_AND_RELEASE.md) | Install toolchains, run verification, deploy Web, or package native apps |

## How it fits together

```text
SvelteKit routes, views and stores
                |
      DatabaseDriver + SecretVaultDriver
                |
          Runtime selection
           /               \
 TauriNativeDriver         WebStandaloneDriver
 Desktop / Mobile          Browser main thread
          |                         |
     Tauri IPC              Dedicated database worker
          |                   /              \
 SQLite / SQLCipher     SQLite WASM / OPFS   WebSecretVault
 + NativeSecretVault           |
          |               Rust web-engine WASM
          |                    |
          +----- core-engine --+
                 Shared finance rules and schema

Optional Cloud Sync: each storage adapter talks to your Turso database.
```

Driver contracts keep platform details out of the UI. The native adapter executes Rust directly against SQLite. The web adapter runs both the shared Rust engine and SQLite as WebAssembly inside a worker, keeping database work off the UI thread. [Driver contracts](../src/lib/drivers/contracts.ts) · [Runtime selection](../src/lib/drivers/runtime.ts) · [Native IPC](../src-tauri/ipc/mod.rs) · [Worker](../src/lib/drivers/web/db.worker.ts) · [Shared core](../crates/core-engine/src/lib.rs)

## Choose a storage mode

| | Local Only (`local-isolated`) | Cloud Sync (`turso-sync`) |
| --- | --- | --- |
| Where edits go | Your local ledger. Ledger storage makes zero outbound requests. | Your local ledger first. Sync sends ledger data to your own Turso database. |
| Offline | Read and write locally. A hosted Web app needs an initial online load to cache its app files. | Existing ledgers remain usable offline. Pending changes are tracked durably and can sync after reconnecting. Enrollment needs a connection. |
| Across devices | Use an encrypted backup to move a copy manually. | Enroll each device with the same Turso database URL and a valid token. Use a separate Turso database for each ledger. |
| Native files | Managed SQLite or optional SQLCipher under Tauri app data. Portable files are also supported. | Same managed local storage. The native vault stores the Turso token in the platform credential store. |
| Browser files | Origin Private File System (OPFS). Encrypted ledgers store an AES-GCM-encrypted image there and open it into memory. | Same origin-private storage. The Turso token is encrypted by Web Crypto with a non-extractable key kept in IndexedDB. |

Quick-unlock secrets are stored only when requested. Local ledger encryption and encrypted backup files protect those copies. Cloud Sync sends relational snapshots over HTTPS that the Turso database can store and read as data. Plan access to that database accordingly. [Native sync](../src-tauri/storage/sync/mod.rs) · [Web sync](../src/lib/drivers/web/sync.ts) · [Web vault](../src/lib/drivers/web/secret-vault.ts) · [OPFS adapter](../src/lib/drivers/web/opfs.ts)

## Five-minute quick start

These steps assume the [prerequisites](BUILD_AND_RELEASE.md#prerequisites-and-toolchain) are installed. A first build can take longer while dependencies compile. Run commands from the repository root.

1. Install locked dependencies and the matching WebAssembly toolchain.

   ```sh
   npm ci
   rustup target add wasm32-unknown-unknown
   cargo install wasm-bindgen-cli --version 0.2.130 --locked
   ```

2. Choose one launch command. Both development commands use port 1420.

   | Runtime | Command | Where it opens |
   | --- | --- | --- |
   | Standalone Web | `npm run dev` | Browser at [http://localhost:1420](http://localhost:1420) |
   | Tauri Desktop | `npm run tauri -- dev` | Financial GPS desktop window |

3. On the welcome screen, keep **IDR** or choose your **Primary currency**. Choose a first account such as **Cash**, give it a name, and enter an optional starting balance. Select **Start Tracking**. Open **Transactions → New transaction** to enter an amount, choose an account and category, then save or add details. **More setup options** lets you open an existing activity file or choose Cloud Sync. [Welcome screen](../src/routes/+page.svelte) · [Account setup](../src/lib/views/AccountSetup.svelte) · [Transaction wizard](../src/lib/views/NewTransaction.svelte)

A browser tab and the Tauri window keep separate copies. In the browser, changing the scheme, host, or port also changes the storage origin. Keep using the same URL to find your ledger.

For engineering history, see the [requirements specification](REQUIREMENTS_SPEC.md), [architecture specification](ARCHITECTURE_SPEC.md), [feature inventory](FEATURE_INVENTORY.md), [validation report](PHASE1_4_VALIDATION_REPORT.md), and [previous release sign-off](V1_RELEASE_SIGN_OFF.md).
