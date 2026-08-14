# Changelog

## Unreleased

- Changed requests that cannot be served because no account is ready, the selected account is disabled, or it is cooling down to return `503` with error type `not_ready`, matching `/readyz`. These previously returned `502 provider_error`, which is now reserved for genuine upstream failures.
- Stopped spending the retry budget on deterministic failures, so a request with no ready account fails immediately instead of after the full backoff sequence.
- Documented that Windows builds must go through `build.ps1`, including the BoringSSL `gcc.exe` error produced by calling cargo directly.

## 0.4.0

- Added weighted multi-account routing, background health recovery, per-account enable/disable controls and automatic failover.
- Added local-only account administration by default, with explicit remote administration opt-in.
- Added Windows DPAPI protection for persisted account credentials and restrictive file permissions on Unix.
- Replaced JSON task persistence with indexed SQLite storage, migration, pagination and retention cleanup.
- Added per-account metrics and configurable provider origins for deterministic protocol integration tests.
- Added automatic Chrome Cookie synchronization for saved Google account profiles.
- Expanded the embedded console with account administration and recent task history.
- Added strict CI, dependency auditing, secret scanning, Dependabot, SBOM generation, provenance attestations and multi-architecture GHCR images.
- Added contributor guidance and GitHub issue and pull-request templates.

## 0.3.0

- Added isolated multi-account runtimes with independent HTTP clients, Cookie jars and Gemini session metadata.
- Added least-inflight round-robin routing and `X-Gemini-Account` affinity.
- Added account cooldown/failover behavior for rate limits, authentication failures and transient provider errors.
- Added persistent account binding for asynchronous generation tasks.
- Added the sanitized `/v1/accounts` status endpoint.
- Added a separate `GEMINI_ADMIN_TOKEN` for Cookie synchronization.
- Reworked account persistence to use a recoverable, atomically replaced local accounts file.
- Updated the embedded console and Chrome extension for named multi-account synchronization.
- Fixed stream concurrency permits so they remain held until the provider stream closes.

## 0.2.0

- Added Responses API, Gemini v1beta compatibility, function calling, the embedded console, Cookie synchronization, protocol retries and cross-platform release workflows.
