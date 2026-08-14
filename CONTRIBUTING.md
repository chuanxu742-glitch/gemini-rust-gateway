# Contributing

## Local checks

Use Rust 1.85 or newer. On Windows, install MSYS2 UCRT64 and run:

```powershell
.\build.ps1 fmt
.\build.ps1 clippy
.\build.ps1 test
.\build.ps1 build -Release
```

On Linux:

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

Also validate `web/app.js`, `chrome-extension/popup.js`, `chrome-extension/background.js` and `chrome-extension/manifest.json` before opening a pull request.

## Protocol fixtures

Gemini Web is an undocumented upstream protocol. Add sanitized fixtures for parser changes. Never commit Cookie values, account identifiers, response headers containing credentials, or unsanitized HTML captured from an authenticated session.

## Pull requests

- Keep compatibility behavior covered by tests.
- Document new environment variables in `.env.example` and `README.md`.
- Update `CHANGELOG.md` for user-visible changes.
- Do not weaken bounded queues, request limits, account isolation, or secret redaction.
