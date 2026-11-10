# Tech Stack

## Frontend
- **React 18** with TypeScript
- **Vite 5** — dev server and bundler
- **Zustand 4** — global state management (`src/store.ts`)
- **Tauri JS API** (`@tauri-apps/api`) — `invoke` for commands, `listen` for events

## Backend (Rust)
- **Tauri 2** — desktop shell, system tray, OS integration
- **AWS SDK for Rust** (`aws-sdk-s3`, `aws-config`) — S3 operations
- **Tokio** — async runtime
- **notify 6** — local file system watcher
- **rusqlite** (bundled) — SQLite for sync state persistence
- **keyring 2** — OS-native credential storage
- **serde / serde_json** — serialization
- **thiserror** — error types
- **chrono** — timestamps
- **glob** — exclusion pattern matching
- **md-5** — ETag/checksum validation

## Testing
- **Vitest** — frontend unit tests (`src/__tests__/`)
- **@testing-library/react** — component tests
- **fast-check** — property-based tests (frontend)
- **proptest** — property-based tests (Rust, `src-tauri/tests/`)
- **tempfile** — temporary files in Rust tests

## Common Commands

```bash
# Frontend dev server (Vite only)
npm run dev

# Type-check + production build
npm run build

# Run frontend tests (single pass, no watch)
npm test

# Run Rust tests
cargo test --manifest-path src-tauri/Cargo.toml

# Full desktop app in dev mode
npm run tauri dev
```

## Configuration Files
- `vite.config.ts` / `vitest.config.ts` — Vite and Vitest setup
- `tsconfig.json` / `tsconfig.node.json` — TypeScript config
- `src-tauri/tauri.conf.json` — Tauri app config (identifier, window, capabilities)
- `src-tauri/Cargo.toml` — Rust dependencies
- App config at runtime: JSON file in OS config dir (`amazon-s3-drive/config.json`)
- Sync DB at runtime: SQLite at `amazon-s3-drive/sync.db`
