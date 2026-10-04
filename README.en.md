# Z·SWITCH (zcode-switch)

[简体中文](README.md) ｜ **English**

A Tauri 2 desktop tool for one-click switching between multiple ZCode accounts, with live quota display. Only the login identity changes — projects, sessions, settings and plugins all stay untouched.

![screenshot](docs/screenshot.png)

## Features

- **Save / switch accounts**: one-click login switching; the current login is auto-preserved before any switch — accounts are never lost
- **Add accounts**: OAuth login for new accounts inside the tool (BigModel / z.ai entries), never touching the current login
- **Quota display**: inline plan quota and reset time per account row, multi-plan grouping
- **Claim promotions**: one-click claim for eligible promotions; "Auto claim" toggle (off by default) checks and claims periodically — manual actions take priority
- **Encrypted import / export**: `.zsb` bundle, PBKDF2(100k) + AES-256-GCM password encryption
- **Bilingual UI (中文 / English)**: one-click switch in Settings — main window, tray, error messages and CLI output all covered; first run follows your OS language
- **Tray / autostart / CLI automation**

## Security Design

- **Local first**: all data stays on your machine; no telemetry, no remote storage; quota queries go directly to official endpoints
- **WebView CSP**: the `script-src` baseline is `'self'`; minimal third-party script/image sources are allowed for the official promotion web component. UI events do not rely on dynamic execution — they go through a whitelist-based dispatcher
- **No account loss**: the current login is auto-preserved before switching if not yet saved; file writes go through a temp file + atomic rename
- **Path traversal protection**: account id whitelist (`[A-Za-z0-9-]`); delete/read cannot escape the account store directory
- **Encrypted export**: PBKDF2-HMAC-SHA256 (100k iterations) + AES-256-GCM, random salt/nonce; wrong password simply fails, no plaintext traces
- **Credentials decrypted locally only**: decryption is used solely to display username/email; export files are password-encrypted

## CLI

```
zcode-switch.exe --cli state|list
zcode-switch.exe --cli quota [--id <account-id>]
zcode-switch.exe --cli claim-preview [--id <account-id>]
zcode-switch.exe --cli capture [--name <name>]
zcode-switch.exe --cli switch --id <id> [--force] [--restart|--no-restart] [--hot <bool>|--no-hot]
zcode-switch.exe --cli kill
zcode-switch.exe --cli export --id <id> --out <a.zsb>
zcode-switch.exe --cli export-all --out <all.zsb>
zcode-switch.exe --cli import --file <file.zsb>
zcode-switch.exe --cli rename|delete|update|behavior|setpath|launch
zcode-switch.exe --cli --lang en state              # English output (--lang takes a space-separated value, works anywhere in the command line; defaults to the GUI/system language)
```

CLI password (export / import): prefer the `ZSW_PASSWORD` environment variable (keeps it out of process lists and command history); `--password <password>` also works.

## FAQ

### macOS asks for Microphone / Accessibility / Screen Recording permission?

Deny all of them — nothing breaks.
The embedded pages (such as the login page) are rendered by the system WebView, which relays their requests as system permission prompts attributed to the app; neither the app itself nor its embedded pages use any of these three capabilities.

## Build

Install Node.js and the Rust toolchain (`cargo` and `rustc`) first. macOS also needs Xcode Command Line Tools (`xcode-select --install`). If Cargo is missing, follow the [official Rust installation guide](https://rust-lang.org/tools/install/), run `source "$HOME/.cargo/env"`, and check `cargo --version`.

```bash
npm install
npm run tauri dev      # development (HMR)
npm run tauri build    # macOS: APP + DMG; Windows: NSIS
npm run dist           # copy installers into release/
```

macOS artifacts are under `src-tauri/target/release/bundle/macos/` and `src-tauri/target/release/bundle/dmg/`. Builds use the host architecture. Install ZCode before use; the app detects standard Applications locations, and settings accept either a `.app` path or its internal executable.

The project CLI detects Cargo in the user directory. Mac packaging skips Finder window customization to avoid depending on Finder automation permissions.

## License

[MIT](./LICENSE)
