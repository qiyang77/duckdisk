# Mac App Store build

DuckDisk uses one repository with two isolated release flavors.

- Direct builds keep Google Drive, the existing system SSH integration, GitHub update checks, and Developer ID distribution.
- Both flavors use the public macOS overlay title bar with native window controls and corners; neither flavor enables Tauri's macOS private API.
- Mac App Store builds omit Google Drive, enable App Sandbox, use the embedded SSH client, require explicit volume/folder selection, and rely on App Store updates.

## Local MAS verification

Build an ad-hoc signed sandboxed app without changing the direct-distribution manifest:

```bash
MAS_SKIP_SIGNING=1 MAS_BUILD_NUMBER=512 npm run release:macos-store
```

The build is written as a ZIP archive to:

```text
src-tauri/target/mas-store/DuckDisk-0.6.1-MAS-unsigned.zip
```

The script builds in an isolated temporary source tree because Tauri 1 statically couples `macos-private-api` to its configuration. This keeps the feature in direct builds and removes it from the MAS binary.

The app is signed and verified in the temporary local directory before it is archived. This avoids the Desktop/iCloud path attaching Finder metadata that invalidates a loose app bundle's signature. Extract the ZIP to `/Applications` or another local directory for manual testing.

## App Store signing

The signed build requires:

```text
MAS_APP_SIGNING_IDENTITY
MAS_INSTALLER_SIGNING_IDENTITY
MAS_TEAM_ID
MAS_PROVISIONING_PROFILE_PATH or MAS_PROVISIONING_PROFILE
MAS_BUILD_NUMBER
```

Run:

```bash
npm run release:macos-store
```

The resulting installer package is written to:

```text
src-tauri/target/mas-store/DuckDisk-0.6.1-MAS.pkg
```

The `Mac App Store` GitHub Actions workflow imports the two distribution
certificates, builds and validates the package, and can upload it to App Store
Connect. With the `submit` input enabled, it also waits for processing, creates
the App Store version, copies localization metadata, sets the supplied
`What's New` text, attaches the build, and submits it to App Review.

Create the App Store Connect app and Mac App Store provisioning profile for the explicit bundle ID `com.duckdisk.app`. The build script decodes the profile and refuses to sign if its bundle ID or Team ID does not match.

## MAS SSH behavior

The MAS SSH client does not execute `/usr/bin/ssh` and does not read `~/.ssh/config`, SSH Agent sockets, or default private-key paths.

- Hosts must use `user@example.com` format.
- Passwords are stored in the MAS-specific Keychain service.
- A private key is selected with the system file picker and copied into the MAS-specific Keychain service.
- The server SHA-256 host-key fingerprint must be confirmed before credentials are saved.
- The existing remote Python 3 scan and deletion scripts are executed through an SSH exec channel.
- Remote deletion remains permanent and is limited to descendants of the configured remote root.

## Suggested App Review notes

Explain these points in App Review notes and provide a test SSH server if review needs to exercise SSH:

- The App Store build intentionally omits Google Drive and all non-App-Store update mechanisms.
- Local access is granted only after the user chooses a folder or volume in the macOS open panel.
- SSH is implemented by the embedded `libssh2` client. The app does not execute `/usr/bin/ssh`, read the user's SSH configuration, or install any code.
- The fixed Python scan/delete commands are bundled app behavior sent to and executed on the SSH server selected by the user; the app does not download or execute remote code on the Mac.
- OneDrive OAuth listens only on a temporary localhost callback port, which is why the sandbox includes the incoming-network entitlement.
