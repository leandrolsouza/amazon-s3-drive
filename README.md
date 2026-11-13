# Amazon S3 Drive

Amazon S3 Drive is a desktop app that keeps a local folder and an Amazon S3 bucket in sync. It runs in the system tray and has a Brazilian Portuguese interface.

## Features

- Choose two-way sync, upload-only backup, or download-only mirroring.
- Limit sync to an S3 prefix or exclude local files using glob patterns.
- Manage multiple bucket and folder pairs and start or pause them independently.
- Queue changes while offline, resolve file conflicts, and review pending deletions.
- Monitor sync status, activity logs, and estimated S3 storage costs.
- Publish static sites from local files to S3.

## Install

Check [GitHub Releases](https://github.com/leandrolsouza/amazon-s3-drive/releases) for installers. The release workflow builds `.msi` and `.exe` installers for Windows, `.dmg` for macOS, and `.AppImage` and `.deb` for Linux. You can also build an installer from source using the commands below.

## Connect a bucket

1. Open **Configurações** (Settings) and enter the bucket name and AWS region.
2. Configure AWS authentication with an access key and secret key, an optional session token for temporary credentials, or the IAM role option. The app stores credentials entered in the interface in the operating system's credential store.
3. Choose a local folder and a sync mode. Set an S3 prefix if you want to sync only part of the bucket.
4. Select **Salvar e iniciar** (Save and start) to begin syncing. Open **Buckets** to configure additional bucket and folder pairs.

Your AWS identity needs permission to list the bucket and perform the object operations used by your sync mode. Before starting, check the selected folder, bucket, and prefix. Sync can change files on the local or remote side depending on the mode.

## Develop and build

Install Node.js 22 (the version used in CI), npm, and the stable Rust toolchain. Follow the [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/) for your operating system.

From the repository root, install dependencies and launch the desktop app in development mode.

```sh
npm ci
npm run tauri dev
```

| Command | Purpose |
| --- | --- |
| `npm run dev` | Run the Vite frontend. Use `npm run tauri dev` for features that need the desktop backend. |
| `npm run build` | Type-check and build the frontend. |
| `npm run tauri build` | Build the desktop app and installers for the host platform. |
| `npm test` | Run the frontend tests with Vitest. |
| `cargo test --manifest-path src-tauri/Cargo.toml` | Run the Rust tests. |

The React and TypeScript frontend lives in [`src/`](src/). The Rust sync engine and Tauri integration live in [`src-tauri/`](src-tauri/). Product requirements and design notes are in the [project specs](.kiro/specs/amazon-s3-drive/).
