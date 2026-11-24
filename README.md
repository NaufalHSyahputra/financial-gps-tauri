# Financial GPS

Financial GPS is a local-first personal finance app for tracking accounts, transactions, budgets, assets, and shared expenses. Run it in a standard browser or as a Tauri app. Keep your activity on one device with **Local Only**, or use **Cloud Sync** with your own Turso database.

## Documentation

| Guide | What it covers |
| --- | --- |
| [Overview and quick start](docs/README.md) | Architecture, storage choices, and running the app locally |
| [Feature guide](docs/FEATURES.md) | Step-by-step workflows and code entry points |
| [Debugging runbook](docs/DEBUGGING.md) | Browser, native desktop, Android, and iOS troubleshooting |
| [Build and release](docs/BUILD_AND_RELEASE.md) | Toolchains, verification, static hosting, and native packaging |

Once the [prerequisites](docs/BUILD_AND_RELEASE.md#prerequisites-and-toolchain) are installed, run `npm ci` and use `npm run dev` for Web or `npm run tauri -- dev` for Desktop. The Web app is available at [http://localhost:1420](http://localhost:1420).
