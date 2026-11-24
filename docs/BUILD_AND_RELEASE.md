# Build and release

[Documentation home](README.md) · [Features](FEATURES.md) · [Debugging](DEBUGGING.md)

Run commands from the repository root unless a step names another directory. Web and native builds share the frontend build. The native packaging tools and signing setup depend on the target platform.

## Prerequisites and toolchain

| Tool | Repository requirement |
| --- | --- |
| Node.js and npm | Node.js 22.17 or newer satisfies the pinned SvelteKit package. Use `npm ci` with the committed lockfile. This checkout was checked with Node.js 26.13.0 and npm 11.20.0. |
| Rust and Cargo | The workspace uses the 2024 edition. There is no repository-wide Rust version pin. Rust 1.100.0 is the checked toolchain. |
| WebAssembly | Install `wasm32-unknown-unknown` and `wasm-bindgen-cli` **0.2.130** to match the Rust dependency. The build script calls Cargo and `wasm-bindgen` directly. `wasm-pack` is unnecessary for this checkout. |
| Tauri | The npm CLI is pinned to **2.12.6** and the Rust runtime to **2.12.7**. Platform SDKs and system libraries come from the build host. |

Sources are the [frontend manifest](../package.json), [workspace](../Cargo.toml), [WASM manifest](../crates/web-engine/Cargo.toml), [WASM build script](../scripts/build-web-engine.mjs), and [native manifest](../src-tauri/Cargo.toml).

1. Install Node.js 26.13.0 with npm if you want to match the checked environment. Install Rust with rustup. [Node.js downloads](https://nodejs.org/en/download) · [Rust installation](https://rustup.rs/)
2. Install the frontend dependencies and the WASM toolchain.

   ```sh
   rustup toolchain install 1.100.0 --profile minimal --component clippy --component rustfmt
   rustup override set 1.100.0
   rustup target add wasm32-unknown-unknown
   cargo install wasm-bindgen-cli --version 0.2.130 --locked
   npm ci
   ```

3. For Tauri or native workspace tests, install your platform's native build dependencies. On Debian or Ubuntu with WebKitGTK 4.1 available, run:

   ```sh
   sudo apt-get update
   sudo apt-get install -y build-essential pkg-config curl wget file \
     libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
     libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev
   ```

   The D-Bus headers support the Linux credential backend. On Arch or CachyOS, use the distribution's Tauri prerequisites and D-Bus development files. On Windows, install Visual Studio Build Tools with **Desktop development with C++** and ensure WebView2 is available. On macOS, install the Xcode command-line tools with `xcode-select --install` for desktop development. [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) · [Native dependencies](../src-tauri/Cargo.toml)

### Android toolchain

1. Install Android Studio and use SDK Manager to install Android SDK Platforms **37** and **36**, Platform-Tools, Build-Tools, Command-line Tools, and **NDK (Side by side)**. The pinned CLI generates an app targeting API 37 and the pinned Rust Tauri Android library compiles against API 36. Use Android Studio's bundled Java runtime or a JDK compatible with the generated Gradle project. [Tauri CLI Android template](https://raw.githubusercontent.com/tauri-apps/tauri/tauri-cli-v2.12.6/crates/tauri-cli/templates/mobile/android/app/build.gradle.kts) · [Tauri Android library](https://raw.githubusercontent.com/tauri-apps/tauri/tauri-v2.12.7/crates/tauri/mobile/android/build.gradle.kts)
2. Set `JAVA_HOME` to the selected JDK, `ANDROID_HOME` to the SDK directory shown in Android Studio, and `NDK_HOME` to its selected `ndk/` version directory. Put the SDK's `platform-tools` directory on your path for `adb`. Follow the platform-specific setup in the [Tauri Android prerequisites](https://v2.tauri.app/start/prerequisites/#android).
3. Install the Rust targets and generate the native project on the build machine.

   ```sh
   rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
   npm run tauri -- android init
   ```

   The checkout has no committed Gradle project or NDK version pin. Record the SDK Build-Tools and NDK versions used for qualification with the release artifacts. [Generated directory policy](../.gitignore)

### iOS toolchain

On macOS, install full Xcode, select it as the active developer directory, and finish its first-launch setup. Install the simulator runtime if needed. Install CocoaPods with Homebrew as shown below. [Tauri iOS prerequisites](https://v2.tauri.app/start/prerequisites/#ios)

```sh
brew install cocoapods
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
npm run tauri -- ios init
```

The targets above cover arm64 devices, Apple Silicon simulators, and Intel simulators. Record the Xcode and SDK versions when qualifying a release.

## Pre-release verification

Run in this order on a machine with the frontend and native prerequisites. Start with installed dependencies from the previous section.

```sh
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo check --locked -p core-engine --target wasm32-unknown-unknown
npm run audit:drivers
npm run check
npm run build
npx playwright install chromium
npm run test:web-driver
npm run test:storage-ui
npm run test:calculator
node tests/web-offline.mjs
```

1. Run the browser checks with the native app stopped. The browser sync tests use a local protocol fixture. Optional authenticated sync and OS vault tests have their own environment requirements, as shown by the Rust test runner.
2. If releasing native packages, run release tests and qualify the installed package on each supported target.

   ```sh
   cargo test --release --workspace
   FINANCIAL_GPS_BENCHMARK=1 cargo test --release -p financial-gps --test phase_three representative_250k_ledger_uses_composite_date_and_fts_indexes -- --nocapture
   ```

3. On a disposable profile, verify first launch, IDR and alternate-currency setup, Cash accounts, transaction wizard, encrypted backup and fresh restore, and vault behavior. For Cloud Sync, test enrollment, write while offline, reconnect, and review conflicting edits. Record the artifact digest, toolchain, target OS, test outcome, and any ignored checks. The [previous Linux sign-off](V1_RELEASE_SIGN_OFF.md) contains earlier native qualification evidence. The current storage features were tested with standalone Chromium and native Rust/IPC. Qualify hosted Turso and each mobile target independently before distributing them.

Schema version 2 adds Cash through [0002_cash_accounts.sql](../crates/core-engine/migrations/0002_cash_accounts.sql). It keeps account IDs, row IDs, and references, and validates foreign keys before commit. Schema version 3 adds nullable category icon identifiers through [0003_category_icons.sql](../crates/core-engine/migrations/0003_category_icons.sql). The first two migrations stay byte-for-byte intact. The current app upgrades version 1 and 2 databases, backups, and sync snapshots on open. Update every sync device before syncing with the new app.

## Web production hosting

1. Run the static build and preview it locally.

   ```sh
   npm run build
   npm run preview
   ```

2. Deploy the contents of `build/` at the root of an HTTPS origin. Serve real assets directly and use `index.html` as the fallback for client routes such as `/transactions`. The service worker is registered at `/service-worker.js`.
3. Apply these headers to the document and worker responses. COOP and COEP enable cross-origin isolation for worker operations. The HTTP CSP limits network destinations and workers. SvelteKit also generates a page CSP that permits the app's WebAssembly startup. Keep the worker policy on the Web host and the native policy in Tauri. [Vite and CSP](../vite.config.ts) · [Hosting headers](../static/_headers) · [Native CSP](../src-tauri/tauri.conf.json)

### Cloudflare Pages

Upload `build/` as the site output. The build already copies the following from `static/_headers`. Use the default SPA fallback by keeping the output root free of a top-level `404.html`. [Pages headers](https://developers.cloudflare.com/pages/configuration/headers/) · [Pages routing](https://developers.cloudflare.com/pages/configuration/serving-pages/)

```text
/*
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Embedder-Policy: require-corp
  Content-Security-Policy: connect-src 'self' https://*.turso.io wss://*.turso.io; object-src 'none'; worker-src 'self'
```

### Vercel

Deploy the prebuilt files from `build/` as a static project with the **Other** framework preset. Place this `vercel.json` alongside the deployed `index.html` and use that directory as the project root. Leave the build command empty when deploying prebuilt files. [Vercel project configuration](https://vercel.com/docs/project-configuration/vercel-json) · [Vercel rewrites](https://vercel.com/docs/routing/rewrites)

```json
{
  "framework": null,
  "outputDirectory": ".",
  "headers": [
    {
      "source": "/(.*)",
      "headers": [
        { "key": "Cross-Origin-Opener-Policy", "value": "same-origin" },
        { "key": "Cross-Origin-Embedder-Policy", "value": "require-corp" },
        {
          "key": "Content-Security-Policy",
          "value": "connect-src 'self' https://*.turso.io wss://*.turso.io; object-src 'none'; worker-src 'self'"
        }
      ]
    }
  ],
  "rewrites": [{ "source": "/(.*)", "destination": "/index.html" }]
}
```

### Nginx

Copy the build output to `/srv/financial-gps`. Add the following inside the HTTPS site's `server` block. Ensure Nginx loads its usual MIME types for JavaScript and CSS. [Nginx routing](https://nginx.org/en/docs/http/ngx_http_core_module.html#try_files) · [Nginx response headers](https://nginx.org/en/docs/http/ngx_http_headers_module.html#add_header)

```nginx
root /srv/financial-gps;
index index.html;

add_header Cross-Origin-Opener-Policy same-origin always;
add_header Cross-Origin-Embedder-Policy require-corp always;
add_header Content-Security-Policy "connect-src 'self' https://*.turso.io wss://*.turso.io; object-src 'none'; worker-src 'self'" always;

location ~ \.wasm$ {
    types { application/wasm wasm; }
    try_files $uri =404;
}

location /_app/ {
    try_files $uri =404;
}

location = /service-worker.js {
    try_files $uri =404;
}

location / {
    try_files $uri $uri/ /index.html;
}
```

Run `sudo nginx -t` before reloading Nginx. On any deployed host, open a deep link directly, inspect the document and worker response headers, and check that WASM requests return `application/wasm`. Create and reopen a disposable ledger, then reload offline after the service worker has installed. Changing the deployment origin creates a separate browser storage scope. [Offline smoke test](../tests/web-offline.mjs)

## Desktop packaging

Run the Tauri build on the target platform. The build calls `npm run build` through `beforeBuildCommand`. The checked-in configuration sets the product name, application identifier, icons, and CSP, and enables bundling. [Tauri config](../src-tauri/tauri.conf.json) · [Tauri distribution](https://v2.tauri.app/distribute/)

| Platform | Bundle command | Default workspace output directories |
| --- | --- | --- |
| Linux | `npm run tauri -- build --bundles deb,appimage` | `target/release/bundle/deb/` and `target/release/bundle/appimage/` |
| Windows | `npm run tauri -- build --bundles msi,nsis` | `target/release/bundle/msi/` for `.msi` and `target/release/bundle/nsis/` for the setup `.exe` |
| macOS | `npm run tauri -- build --bundles app,dmg` | `target/release/bundle/macos/` for `.app` and `target/release/bundle/dmg/` for `.dmg` |

`npm run tauri -- build` uses the platform's default bundles. Use `npm run tauri -- build --no-bundle` for a release executable only. A specific Cargo target or custom target directory changes the artifact path. [Tauri CLI options](https://v2.tauri.app/reference/cli/#build)

### Linux baseline

Build on the oldest distribution you intend to support that provides WebKitGTK 4.1. Debian 12 and Ubuntu 22.04 are Tauri baseline examples. A package built on a rolling release may link to an OpenSSL or glibc version missing on an older host. [Debian packaging](https://v2.tauri.app/distribute/debian/) · [AppImage packaging](https://v2.tauri.app/distribute/appimage/) · [Previous Linux linkage](V1_RELEASE_SIGN_OFF.md)

With Docker, the following opens a Debian 12 build shell with Node installed. Use a dedicated clean checkout for container output.

```sh
docker run --rm -it \
  --mount type=bind,source="$PWD",target=/work \
  --workdir /work node:26.13.0-bookworm bash
```

Inside the root shell, run the Debian package setup above without `sudo`, install rustup, and apply the same pinned toolchain setup. Then build the native packages and inspect linkage.

```sh
npm run tauri -- build --bundles deb,appimage
ldd target/release/financial-gps
find target/release/bundle -type f \( -name '*.deb' -o -name '*.AppImage' \)
```

If AppImage creation requires FUSE that the container does not provide, build the AppImage on a baseline VM with FUSE support. Test actual installation and launch on a clean target OS. On Debian, inspect the resulting dependency declarations with `dpkg-deb -I` and verify that the package resolves every required system library.

### Windows and macOS distribution

The checkout includes PNG and ICO icons. To prepare a macOS release icon, generate the platform icon set with `npm run tauri -- icon src-tauri/icons/icon.png` in a release checkout and add `icons/icon.icns` to the bundle icon list. Check app icons on each target, including the generated mobile projects. [Tauri app icons](https://v2.tauri.app/develop/icons/) · [Bundle configuration](../src-tauri/tauri.conf.json)

- **Windows** builds MSI with WiX and setup executables with NSIS. Install and launch both selected installer types on clean Windows machines. For public distribution use your code-signing certificate or signing service, then verify the installer signature. [Windows installers](https://v2.tauri.app/distribute/windows-installer/) · [Windows signing](https://v2.tauri.app/distribute/sign/windows/)
- **macOS** builds on a Mac. For a universal artifact, install both desktop targets and build with the universal target below. For direct distribution, use a Developer ID Application identity, sign, notarize, and staple using the Tauri workflow. Verify the signed app on a clean Mac. [macOS signing and notarization](https://v2.tauri.app/distribute/sign/macos/)

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri -- build --target universal-apple-darwin --bundles app,dmg
```

## Mobile release builds

Android and iOS projects are generated locally. Keep signing keys and credentials in your platform's signing workflow and out of source control. Tauri uses the same frontend distribution for these builds. [App config](../src-tauri/tauri.conf.json) · [Tauri mobile commands](https://v2.tauri.app/reference/cli/#android)

### Android APK and AAB

Build an installable debug APK for a physical ARM64 device from the repository root. Gradle signs it with the Android debug keystore. Add `x86_64` to the Tauri target list when building for an x86-64 emulator.

```sh
npm run tauri -- android build --debug --apk --target aarch64
find src-tauri/gen/android/app/build/outputs/apk -type f -name '*.apk'
```

Android builds vendor OpenSSL for SQLCipher and link the app library with 16 KiB page alignment. Set `NDK_HOME` to an installed Android NDK before building. The SDK manager lists the Android 17 platform as `platforms;android-37.0`. [Android page-size guidance](https://developer.android.com/guide/practices/page-sizes)

For a signed distribution build, configure the release key before bundling.

1. Initialize the [Android toolchain](#android-toolchain). Generate an upload key once, and store the keystore and recovery information securely. The command prompts for the password.

   ```sh
   keytool -genkeypair -v -keystore financial-gps-upload.jks \
     -alias financial-gps-upload -keyalg RSA -keysize 2048 -validity 10000
   ```

2. Move the keystore outside the repository. Create `keystore.properties` in the generated `src-tauri/gen/android/` project, using the real keystore path, `keyAlias` of `financial-gps-upload`, and the password. Add a release signing configuration to the generated app's `build.gradle.kts` that loads `storeFile`, `keyAlias`, and `password`, then assign it to the release build type. Use the example for the generated Kotlin DSL project. [Tauri Android signing](https://v2.tauri.app/distribute/sign/android/)
3. Build the APK and AAB and locate the outputs beneath the generated Gradle project.

   ```sh
   npm run tauri -- android build --apk --aab
   find src-tauri/gen/android/app/build/outputs -type f \( -name '*.apk' -o -name '*.aab' \)
   ```

4. Install the signed APK on a test device and verify launch and storage. Use the signed AAB for the Google Play release flow. [Tauri Google Play guide](https://v2.tauri.app/distribute/google-play/)

### iOS IPA

1. Initialize the [iOS toolchain](#ios-toolchain). In Apple Developer and App Store Connect, register the bundle identifier from the Tauri config. In Xcode, choose the correct team and distribution certificate, and select a provisioning profile that covers the app's entitlements. [Tauri iOS signing](https://v2.tauri.app/distribute/sign/ios/)
2. Build for App Store Connect using the CLI, or open the generated project in Xcode and use **Product → Archive** with an iOS device destination. Export from Organizer using the intended distribution method.

   ```sh
   npm run tauri -- ios build --export-method app-store-connect
   find src-tauri/gen/apple/build -type f -name '*.ipa'
   ```

3. Check the signed export and test an installed build on a physical device or through TestFlight. [Tauri App Store guide](https://v2.tauri.app/distribute/app-store/)
