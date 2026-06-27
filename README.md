# Container UI

Lightweight macOS desktop console for the local [`apple/container`](https://github.com/apple/container) CLI.

## Features

- Shows `container system status`, CLI version, local containers, images, volumes, networks, and one-shot stats.
- Displays container details from `container inspect` and recent logs from `container logs`.
- Starts containers from the container table.
- Stops containers only after a backend-issued approval challenge is confirmed in the UI.
- Writes a local command activity log under the app data directory.
- Uses a browser-preview mock when the frontend runs outside Tauri, so UI layout can be checked with Vite.

## Requirements

- macOS with Apple silicon.
- `apple/container` installed at one of the supported paths:
  - `/usr/local/bin/container`
  - `/opt/homebrew/bin/container`
  - `/usr/bin/container`
- Node.js 22+.
- Rust/Cargo.

## Development

```bash
npm install
npm run typecheck
npm run lint
npm test
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri:dev
```

The Vite-only preview is useful for layout checks:

```bash
npm run dev -- --host 127.0.0.1
```

The debug build may use `CONTAINER_UI_CONTAINER_BIN`, but only when it points to one of the supported fixed paths above. Release builds ignore this override.

## Mise Tasks

Common local commands are also available through `mise`:

- `mise run local-dev`: run the Vite browser preview.
- `mise run local-check`: run npm audit, npm registry signature checks, TypeScript, lint, frontend tests, Rust tests, and frontend build.
- `mise run local-package`: build the packaged macOS Tauri app.

## Packaging

```bash
npm run tauri:build
```

Build artifacts are generated under:

- `src-tauri/target/release/bundle/macos/Container UI.app`
- `src-tauri/target/release/bundle/dmg/Container UI_0.1.0_aarch64.dmg`

## Security Model

The desktop backend uses `std::process::Command` with fixed `container` argument sets. The frontend cannot submit arbitrary command strings.

Allowed operations:

- Read-only: `--version`, `system status`, `list --all`, `image list`, `volume list`, `network list`, `stats --no-stream`, `inspect`, `logs`
- Mutating: `start`, `stop` for non-managed containers

Forbidden operations include `run`, `create`, `delete`, `exec`, `shell`, `prune`, image pull/push/remove, volume create/remove, network create/remove, plugin operations, and arbitrary command strings.

Guardrails:

- Container IDs are validated before they are passed as CLI arguments.
- The child process environment is cleared and rebuilt with a small allow-list.
- CLI stdout/stderr is read through bounded streaming buffers before it is returned to the app.
- Stop requests require a fresh in-memory approval challenge, the phrase `STOP <container-id>`, and a human-readable reason of 4 to 180 characters.
- Stop approval challenges expire after 120 seconds and are consumed after one successful validation.
- Containers identified as apple/container managed resources, such as `buildkit`, are visually marked and cannot be stopped from Container UI.
- Visible logs, inspect text, command errors, and activity stderr mask lines that look like secrets.
- Stop approval requests and stop executions require activity log writes. The backend keeps the latest 150 activity records and surfaces corrupt log lines as failed `activity_corrupt_line` records.

Activity is stored at:

```text
~/Library/Application Support/app.yoshhiide.container-ui/activity.jsonl
```
