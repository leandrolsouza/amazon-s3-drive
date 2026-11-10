# Product: Amazon S3 Drive

A desktop application that creates a virtual drive mapped to an Amazon S3 bucket, enabling bidirectional file synchronization between a local folder and S3. Inspired by Google Drive, it runs in the system tray, monitors both local and remote changes, and handles offline queuing and conflict resolution transparently.

## Core Features
- Bidirectional sync between a local folder and an S3 bucket
- File watcher for local changes + polling S3 for remote changes (up to 60s interval)
- Offline operation: queues up to 10,000 pending operations and flushes on reconnect
- Conflict resolution: preserves both versions with a timestamped suffix `[name](conflict-YYYY-MM-DD-HH-MM-SS).ext`
- Selective sync via glob exclusion patterns and S3 prefix filtering
- Multipart upload for files > 100MB (minimum 5MB per part)
- MD5/ETag integrity check on download
- System tray icon with animated sync state
- Credentials stored in OS native keychain (Keychain / Credential Manager / Secret Service)
- Supports standard AWS credentials, STS session tokens, and IAM roles

## UI
Three views accessible via a sidebar:
1. **Status de Sincronização** — file list with per-file sync states
2. **Configurações** — AWS credentials + bucket/path config
3. **Logs & Diagnóstico** — sync log viewer

## Language
The UI and user-facing strings are in Brazilian Portuguese (pt-BR).
