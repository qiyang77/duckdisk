# DuckDisk

<p align="center">
  <img src="docs/assets/duckdisk-icon.png?v=0.6.2-media-6" alt="DuckDisk logo" width="144" height="144">
</p>

[![license](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue)](LICENSE)
[![release](https://img.shields.io/github/v/release/qiyang77/duckdisk?label=release)](https://github.com/qiyang77/duckdisk/releases)
[![website](https://img.shields.io/badge/website-duckdisk-blue)](https://duckdisk.com/)
[![Mac App Store](https://img.shields.io/badge/Mac_App_Store-Download-black?logo=apple)](https://apps.apple.com/app/duckdisk/id6798893880?mt=12)
![platform](https://img.shields.io/badge/platform-macOS-black)
![arch](https://img.shields.io/badge/arch-arm64-lightgrey)
![stack](https://img.shields.io/badge/stack-Tauri%20%7C%20Rust%20%7C%20React-orange)

**DuckDisk** is an informative, table-first storage analyzer for macOS inspired by **WizTree-style** workflows. It scans disks, folders, OneDrive, Google Drive, and SSH paths, then keeps directory sizes, allocated space, parent percentages, file and folder counts, and file-type totals visible in one dense tree view.

The app is built with Tauri, Rust, React, and the `pdu` scanner.

Website: https://duckdisk.com/

## What's New in v0.6.2

- Fixed freezes during scanning, navigation, and app shutdown.
- Corrected Google Drive totals for shared files and disabled unavailable Trash actions.

## Screenshots

### Scan a Local Disk

![DuckDisk local disk scan demo](docs/screenshots/local-scan.gif?v=0.6.2-media-6)

### Drag to Remove Files

![DuckDisk drag-to-remove demo](docs/screenshots/drag-remove.gif?v=0.6.2-media-6)

## Features

- **OneDrive cloud analysis** using file metadata only, without downloading file contents.
- **Google Drive analysis** with OAuth, metadata-only scanning, incremental change tracking, and move-to-Trash cleanup.
- **SSH remote path analysis** through macOS `ssh`, existing keys, and `~/.ssh/config`.
- **Fast incremental cloud refreshes** using Microsoft Graph delta updates and a local metadata cache.
- **Recoverable cloud cleanup** that moves selected OneDrive and Google Drive items to provider trash.
- **Dense tree view** with folder/file counts, sizes, allocated size, and parent percentage.
- **Virtualized large directories** that keep local and remote result tables responsive.
- **Drag-to-delete** cleanup for local, OneDrive, and Google Drive scan results.
- File type summary with extension totals and percentages.
- Finder integration for revealing files and folders.

## Permissions

For accurate full-disk scans, grant DuckDisk Full Disk Access:

1. Open DuckDisk.
2. Click `Grant Full Disk Access`.
3. Enable DuckDisk in macOS Privacy & Security settings.
4. Restart DuckDisk and rescan.

If macOS prompts for permissions during a scan, denied or previously blocked reads may count as scan errors. After granting permissions, run `Rescan` for cleaner results.

OneDrive scans use Microsoft account authorization and request `Files.ReadWrite` so selected files and folders can be moved to the OneDrive Recycle Bin. DuckDisk does not permanently delete cloud items. Refresh tokens are stored in macOS Keychain; cached scan metadata is stored in DuckDisk's application cache.

Google Drive scans request the full `drive` scope because Google does not allow narrower metadata permissions to move arbitrary existing user-selected items to Trash. DuckDisk uses this permission only to read file metadata and perform Trash actions the user explicitly starts; file contents are not downloaded for analysis. Tokens are stored in macOS Keychain, and the app can revoke Google authorization and remove its local account cache. SSH scans use the system `ssh` command with either existing SSH keys/configuration or a password stored in macOS Keychain. DuckDisk can permanently delete user-selected SSH files within the configured remote path only after an additional confirmation.

See the [Privacy Policy](https://duckdisk.com/privacy.html) and [Terms of Use](https://duckdisk.com/terms.html).

## Installation

Choose the distribution that fits your workflow:

| | [Mac App Store](https://apps.apple.com/app/duckdisk/id6798893880?mt=12) | [Direct download](https://github.com/qiyang77/duckdisk/releases/latest) |
| --- | --- | --- |
| Local scans | User-selected folders and volumes | Full-disk and folder scanning with Full Disk Access |
| OneDrive | Yes | Yes |
| Google Drive | Not included | Yes |
| SSH | Sandboxed embedded SSH client | System `ssh`, existing keys, and `~/.ssh/config` |
| Updates | Mac App Store | GitHub releases |

### Mac App Store

[Download DuckDisk from the Mac App Store](https://apps.apple.com/app/duckdisk/id6798893880?mt=12). This edition uses App Sandbox and asks you to choose the local folders or volumes it may access.

### Direct download

Download the latest `.dmg` from [GitHub Releases](https://github.com/qiyang77/duckdisk/releases/latest), open it, and drag DuckDisk into Applications.

Direct-download releases are signed with a Developer ID certificate and
notarized by Apple. macOS can verify the downloaded app before its first launch.

Local development builds may use ad-hoc signing.

## Development

```bash
npm install
npm run build
npm run tauri build
```

### OAuth development setup

DuckDisk's production Google Drive and OneDrive client IDs are public desktop-app
identifiers stored in `src-tauri/src/oauth_config.rs`. Both providers use the
Authorization Code flow with PKCE. OneDrive does not require a client secret.

For a custom OneDrive integration, create a Microsoft Entra app registration for
personal Microsoft accounts and work/school accounts:

1. Configure it as a public mobile/desktop client.
2. Add `http://localhost` as a redirect URI.
3. Add delegated Microsoft Graph permission `Files.ReadWrite`.
4. Do not create or embed a client secret.

Override the production Application (client) ID when running or building DuckDisk:

```bash
DUCKDISK_ONEDRIVE_CLIENT_ID=your-client-id npm run tauri dev
DUCKDISK_ONEDRIVE_CLIENT_ID=your-client-id npm run tauri build
```

For a custom Google Drive integration, create a Google OAuth client with application
type **Desktop app**, enable the Google Drive API, and add your Google account as a
test user while the consent screen remains in testing. Google's token endpoint
currently requires the generated desktop client secret for this client. Supply it
only at build time and optionally override the public client ID:

```bash
DUCKDISK_GOOGLE_CLIENT_ID=your-client-id \
DUCKDISK_GOOGLE_CLIENT_SECRET=your-client-secret \
npm run tauri dev

DUCKDISK_GOOGLE_CLIENT_ID=your-client-id \
DUCKDISK_GOOGLE_CLIENT_SECRET=your-client-secret \
npm run tauri build
```

The macOS installer is generated under:

```text
src-tauri/target/release/bundle/dmg/
```

## Acknowledgements

We thank SquirrelDisk (https://github.com/adileo/squirreldisk), whose work this project refers to.
