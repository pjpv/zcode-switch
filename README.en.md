# Z·SWITCH (zcode-switch)

[简体中文](README.md) ｜ **English**

A Tauri 2 desktop tool for one-click switching between multiple ZCode accounts, with live quota display. Only the login identity changes — projects, sessions, settings and plugins all stay untouched.

![screenshot](docs/screenshot.png)

## Features

- **Save / switch accounts**: snapshots the credentials + config file pair, atomic replacement; the current login is auto-preserved before any switch — accounts are never lost
- **Add accounts**: OAuth login for new accounts inside the tool (BigModel / z.ai entries), never touching the current login
- **Quota display**: inline plan quota and reset time per account row, multi-plan grouping, staggered polling
- **Claim promotions**: one-click claim for eligible promotions (GUI captcha)
- **Encrypted import / export**: `.zsb` bundle, PBKDF2(100k) + AES-256-GCM password encryption
- **Bilingual UI (中文 / English)**: one-click switch in Settings — main window, tray, error messages and CLI output all covered; first run follows your OS language
- **Tray / autostart / CLI automation**

## Security Design

- **Local first**: all data stays on your machine; no telemetry, no remote storage; quota queries go directly to official endpoints
- **WebView CSP (honest disclosure)**: the `script-src` baseline is `'self'`, but `'unsafe-inline'`, `'unsafe-eval'` and `o.alicdn.com` / `*.alicdn.com` are allowed to load the Aliyun captcha SDK (a hard requirement of the claim feature, same source as the ZCode client); `connect-src` whitelists only `*.aliyuncs.com`, `*.aliyun.com`, `ynuf.aliapp.org` (captcha device-fingerprint domain) and alicdn; `img-src` allows `https:` only because captcha popup asset domains are not fixed (image resources only, no script execution). UI interaction does not rely on `eval` — events go through a whitelist-based dispatcher
- **style-src exemption note**: `dangerousDisableAssetCspModification: ["style-src"]` prevents Tauri from appending style hashes — under the CSP spec a hash makes `'unsafe-inline'` ignored, which would block all `style=""` inline attributes and break rendering
- **No account loss**: the current login is auto-preserved before switching if not yet saved; file writes go through a temp file + atomic rename
- **Path traversal protection**: account id whitelist (`[A-Za-z0-9-]`); delete/read cannot escape the account store directory
- **Encrypted export**: PBKDF2-HMAC-SHA256 (100k iterations) + AES-256-GCM, random salt/nonce; wrong password simply fails, no plaintext traces (unit-test asserted)
- **Credentials decrypted locally only**: enc:v1 decryption is used solely to display username/email; export files are password-encrypted

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

## Build

```bash
npm install
npm run tauri dev      # development (HMR)
`npm run dev          # frontend-only preview (open preview.html / preview-settings.html in a browser, mocked data, no Tauri needed)`
npm run tauri build    # NSIS installer
cd src-tauri && cargo test   # unit tests (incl. node↔Rust cross-language crypto vectors)
```

Windows-first (path detection / process management / tray are all Win32 semantics).

## License

[MIT](./LICENSE)
