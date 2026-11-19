# Scripts

Utility scripts for the Lap project. Sections 1–2 and the downloader tests are upstream's; sections 3–5 are this fork's additions.

## 1. download_models

Downloads AI model files (CLIP + InsightFace) required for the app to `src-tauri/resources/models/`. Both the Bash and the PowerShell script pin model revisions and verify SHA-256 before reusing cached files or publishing new downloads; a mismatch stops the build.

### Usage

**macOS / Linux:**
```bash
./scripts/download_models.sh
```

**Windows (PowerShell):**
```powershell
.\scripts\download_models.ps1
```

### Models Downloaded

| Model | Source | Purpose |
|-------|--------|---------|
| `tokenizer.json` | OpenAI CLIP ViT-B/32 | Text tokenization |
| `text_model.onnx` | CLIP ViT-B/32 (quantized) | Text embedding |
| `vision_model.onnx` | CLIP ViT-B/32 (quantized) | Image embedding |
| `det_500m.onnx` | InsightFace Buffalo-S | Face detection |
| `w600k_mbf.onnx` | InsightFace Buffalo-S | Face recognition |

## 2. download_ffmpeg_sidecar

Downloads FFmpeg and FFprobe sidecar binaries for the current platform into `src-tauri/resources/ffmpeg/`. This ensures the packaged app can include the sidecar binaries with releases built from source.

The Bash script pins numeric GitHub asset IDs and verifies SHA-256 for both existing Linux architectures, including pre-staged files. Other platforms retain their existing release URLs without checksum verification. Fresh downloads fail on HTTP errors and use temporary files before promotion. See [build-input provenance and checksum maintenance](../contrib/aur/README.md); this does not add Arch ARM packaging support.

### Usage

**macOS / Linux:**
```bash
./scripts/download_ffmpeg_sidecar.sh
```

**Windows (PowerShell):**
```powershell
.\scripts\download_ffmpeg_sidecar.ps1
```

### Sidecar Files Downloaded

| Platform | Files |
|----------|-------|
| macOS Intel | `ffmpeg-x86_64-apple-darwin`, `ffprobe-x86_64-apple-darwin` |
| macOS Apple Silicon | `ffmpeg-aarch64-apple-darwin`, `ffprobe-aarch64-apple-darwin` |
| Linux x86_64 | `ffmpeg-x86_64-unknown-linux-gnu`, `ffprobe-x86_64-unknown-linux-gnu` |
| Linux aarch64 | `ffmpeg-aarch64-unknown-linux-gnu`, `ffprobe-aarch64-unknown-linux-gnu` |
| Windows x86_64 | `ffmpeg-x86_64-pc-windows-msvc.exe`, `ffprobe-x86_64-pc-windows-msvc.exe` |

## Downloader integrity regression tests

Run `python -m unittest discover` from the repository root to discover the downloader suite. For verbose downloader-only output, use `python -m unittest discover -s scripts/tests -v`.
These tests execute the actual Bash downloaders in temporary layouts, replacing only the network transport to exercise corrupt caches, corrupt successful responses, and failed transfers. Real upstream clean-download/cache smoke evidence is recorded in the Phase 1 verification artifact under `.context/`.

`.github/workflows/test-downloaders.yml` runs Bash syntax checks and the downloader-only suite on Linux and macOS for pull requests and `main` pushes that change `scripts/**` or that workflow. It also supports manual dispatch. The suite uses no upstream downloads or build secrets.

## 3. build-windows (fork)

Builds the three Windows binaries of this fork:

| Binary | Cargo invocation | Purpose |
|---|---|---|
| `lappp.exe` | `cargo build -p lappp` (default `embedded`) | Full desktop app: embeds `lappp-core`, local library, indexing, AI |
| `lappp-client.exe` | `cargo build -p lappp --no-default-features --features client` | Thin client: no `lappp-core`, connects to a remote `lappp-server` |
| `lappp-server.exe` | `cargo build -p lappp-server` | Headless server (axum) — also hosts the Web/PWA build |

Cargo names both desktop variants `lappp.exe`, so the client build is copied under its own name instead of renaming the Cargo target.

### Usage

```powershell
pwsh -File scripts/build-windows.ps1 [-BuildProfile release|debug]
```

- `-BuildProfile release` (default) → `build\windows\` — product build.
- `-BuildProfile debug` → `build\windows\debug\` — debugging/testing build.

The two profiles use **different data directories** (`com.julyx10.lap` vs `com.julyx10.lap.debug`), so a debug build can never clobber the production library and the two output sets never overwrite each other.

The script also forces `CARGO_HTTP_MULTIPLEXING=false` and `CARGO_HTTP_VERSION=HTTP/1.1` (sparse index hangs otherwise behind a proxy).

## 4. typecheck-gate (fork)

Baseline gate around `vue-tsc --noEmit`. It fails on **new** type errors only; existing ones are recorded in `src-vite/typecheck-baseline.json` keyed by `file :: TScode :: normalized message`. The baseline is currently **0** — the gate is a hard zero-error check.

### Usage

```bash
cd src-vite
npm run typecheck:gate          # fail on new errors
node ../scripts/typecheck-gate.mjs --update   # re-baseline (emergency only)
```

## 5. check-cross-refs / run-frontend-tests / smoke-desktop (fork)

| Script | npm alias | What it proves |
|---|---|---|
| `check-cross-refs.mjs` | `npm run test:crossrefs` | Three closed sets: frontend `invoke('literal')` ↔ `generate_handler!` (both feature variants), i18n literals/dynamic prefixes ↔ `locales/en.json`, shortcut action ids ↔ `SHORTCUTS`. Any dangling reference exits 1. |
| `run-frontend-tests.mjs` | `npm run test` | Runs every `test:*` domain in order, prints PASS/FAIL per domain, exits 1 on any failure. |
| `smoke-desktop.mjs` | `npm run smoke:desktop` | Real-binary smoke: boots the desktop build against a real data directory, asserts over WebView2 CDP that the grid paints thumbnails, that they survive scrolling, that a real double-click decodes the original in the viewer, and that the console is error-free. It temporarily patches `additionalBrowserArgs` into `src-tauri/tauri.windows.conf.json` and restores the exact original bytes in a `finally` — **a build made with the debug port must never ship**. |

### Usage

```bash
cd src-vite
npm run test:crossrefs
npm run test
npm run smoke:desktop
```

Environment variables for the smoke gate: `LAP_SMOKE_EXE`, `LAP_SMOKE_DATA_DIR`, `LAP_SMOKE_MIN_WIDTH`, `LAP_SMOKE_PORT`.

Other fork helpers in this directory: `merge-locales.mjs` (merge locale keys across an upstream merge), `rebrand-locales.py` (`Lap` → `lappp` in locale string values), `add-vision-check-i18n.py`, and `service/` (systemd / launchd / WinSW templates for `lappp-server`).
