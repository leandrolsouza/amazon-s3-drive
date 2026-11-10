# Project Structure

## Top-level

```
amazon-s3-drive/
├── src/                  # React/TypeScript frontend
├── src-tauri/            # Rust backend (Tauri)
├── index.html            # Vite entry point
├── package.json
├── vite.config.ts
├── vitest.config.ts
├── tsconfig.json
└── tsconfig.node.json
```

## Frontend — `src/`

```
src/
├── main.tsx              # React entry point
├── App.tsx               # Root component: layout, sidebar, tab routing, status bar
├── store.ts              # Zustand store — single source of truth for all UI state
├── types.ts              # Shared TypeScript types + pure utility functions
├── styles.css            # Global CSS (custom properties, theming dark/light)
├── test-setup.ts         # Vitest setup
├── components/
│   └── NotificationHandler.tsx   # Renders conflict/error/completion banners
├── hooks/
│   └── useSyncEvents.ts          # Subscribes to all Tauri backend events
├── pages/
│   ├── ConfigurationPage.tsx     # AWS credentials + bucket/path config form
│   ├── StatusView.tsx            # File list with sync states + storage analytics
│   └── LogsView.tsx              # Log viewer with filtering
└── __tests__/
    ├── ConfigurationPage.test.tsx
    ├── NotificationHandler.test.tsx
    ├── StatusView.test.tsx
    └── properties.test.ts        # Property-based tests (fast-check)
```

### Frontend Patterns
- All backend calls use `invoke<T>(command, args)` from `@tauri-apps/api/core`
- All backend events are consumed in `useSyncEvents.ts` via `listen`; results are written to the Zustand store
- Components read state exclusively from `useSyncStore`; they never call `invoke` directly except for user-triggered actions
- Types are mirrored exactly from Rust models in `src/types.ts` — keep both sides in sync
- Theme is toggled via `data-theme` attribute on `<html>` and persisted to `localStorage`

## Backend — `src-tauri/src/`

```
src-tauri/src/
├── main.rs               # Binary entry point (calls lib::run)
├── lib.rs                # Tauri setup, AppState, all #[tauri::command] handlers
├── models.rs             # All shared data types and domain error enums
├── config_parser.rs      # Read/write JSON config file (OS config dir)
├── credentials_manager.rs# OS keychain store/load/delete + format validation
├── s3_client.rs          # AWS S3 operations (upload, download, delete, multipart, presign)
├── sync_engine.rs        # Orchestrator: drives upload/download/conflict resolution
├── file_watcher.rs       # Local FS watcher (notify crate)
├── conflict_resolver.rs  # Conflict detection and copy-with-suffix logic
├── queue_manager.rs      # SQLite-backed persistent operation queue
├── db.rs                 # SQLite connection + schema migrations
├── status_monitor.rs     # Per-file state tracking + log file writer
├── network_detector.rs   # Online/offline detection
└── tray.rs               # System tray icon and menu setup
```

### Backend Patterns
- `AppState` (held by Tauri) owns `Arc<SyncEngine>`, `Arc<CredentialsManager>`, `Arc<ConfigParser>`, and `Database`
- Each module owns its own domain error type defined in `models.rs`; all errors convert to `SyncError` via `From` impls
- Tauri commands return `Result<T, String>` — errors are `.to_string()` at the command boundary
- Backend emits typed events to the frontend using `app_handle.emit(event_name, payload)`
- `models.rs` is the single source of truth for all serialized types; never define serializable structs elsewhere
- Credentials are **never** written to disk or logged; always stored via the OS keychain

## Tauri IPC Contract

| Direction | Mechanism | Naming |
|---|---|---|
| Frontend → Backend | `invoke("snake_case_name", { args })` | `snake_case` |
| Backend → Frontend | `app_handle.emit("namespace:event-name", payload)` | `namespace:kebab-case` |

Key events emitted by backend:
- `sync:file-status-changed` — `FileStatusEvent`
- `sync:error` — `SyncErrorEvent`
- `sync:initial-complete` — `InitialCompleteEvent`
- `sync:transfer-progress` — `TransferProgressEvent`
- `conflict:detected` — `ConflictEvent`
- `network:online` / `network:offline`
- `queue:full`
- `credentials:expired` / `credentials:invalid`
- `tray:navigate` — `"status" | "config"`

## Spec & Docs — `.kiro/`

```
.kiro/
├── steering/             # Always-included AI context (this file and siblings)
└── specs/amazon-s3-drive/
    ├── requirements.md   # Product requirements (pt-BR)
    ├── design.md         # Architecture design
    └── tasks.md          # Implementation tasks
```
