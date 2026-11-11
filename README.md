# Keep Sticky Notes

[![Licence: MIT](https://img.shields.io/badge/licence-MIT-green.svg)](LICENSE)
![Platform: Windows 10/11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-blue.svg)
![Architecture: x64 | ARM64](https://img.shields.io/badge/architecture-x64%20%7C%20ARM64-orange.svg)
![Release: v1.1.0](https://img.shields.io/badge/release-v1.1.0-informational.svg)

Native Windows sticky notes that synchronise with Google Keep.

Keep Sticky Notes puts your Google Keep notes on the Windows desktop as small, floating
notes. It is written in Rust against the Win32 API, so each note is an ordinary lightweight
window rather than a browser tab: the whole application idles at roughly 24 MB of memory
with no measurable processor use. Native builds are provided for both x64 and ARM64
(Snapdragon) machines.

> **Important.** Google offers no public Keep API for personal accounts. This application
> uses an **unofficial, reverse-engineered protocol**. It may stop working without notice,
> its use may not be permitted by Google's terms of service, and an account could be
> challenged or restricted. Use it at your own risk, ideally with a secondary account
> first. See [Disclaimer](#disclaimer).

## Key features

- **Floating desktop notes** with drag, resize, always-on-top, adjustable opacity and
  roll-up to the title bar.
- **Text notes and checklists**, with Keep's twelve note colours.
- **Lightweight formatting** for text notes: `**bold**`, `*italic*`, `__underline__`,
  `~~strikethrough~~`, `# heading` and bullets. The markers are stored as plain text, so
  notes remain readable in Keep on the web and on a phone.
- **Themes:** OLED Black (default), OLED Vibrant and Light.
- **Note Manager, tray icon and a global hotkey** (`Win+Alt+N`) for a new note.
- **Two-way synchronisation with safeguards:**
  - synchronisation starts read-only, and nothing is sent to Keep until you enable
    two-way sync;
  - a daily local backup is taken before synchronising, with fourteen kept;
  - bulk deletions or modifications pause and ask for confirmation;
  - when a note was edited in both places, the local version is kept as a conflict copy
    and never silently discarded;
  - local notes are bound to one account and cannot be uploaded to another.
- **Credentials kept out of files.** The Google token is held only in Windows Credential
  Manager. Your password is typed only into Google's own sign-in page.
- **Small and self-contained.** A portable `.exe` of about 2 MB with a statically linked C
  runtime, or an MSIX installer.

## Installation

Download the files for your architecture from the
[latest release](../../releases/latest).

### Portable executable

Download `KeepStickyNotes-x64-portable.exe` (Intel/AMD) or
`KeepStickyNotes-arm64-portable.exe` (Snapdragon) and run it. Nothing is installed.
Windows SmartScreen may warn about an unrecognised application, as the builds are not
signed with a commercial certificate.

### MSIX installer

The MSIX packages are signed with a self-signed certificate, which Windows must trust
before it will install them.

1. Download the `.msix` for your architecture and
   `KeepStickyNotes-signing-certificate.cer`.
2. In an elevated PowerShell window, trust the certificate:

   ```powershell
   Import-Certificate -FilePath .\KeepStickyNotes-signing-certificate.cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople
   ```

3. Double-click the `.msix` file and choose **Install**.

The Microsoft Edge WebView2 Runtime is required for Google sign-in. It is included with
Windows 11 and with current Windows 10.

## Building from source

### Prerequisites

- Windows 10 or 11.
- [Rust](https://rustup.rs/) (stable) with the MSVC targets you need:

  ```powershell
  rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc
  ```

- Visual Studio Build Tools 2022 with the C++ workload (add the ARM64 build tools for
  ARM64 output) and a Windows 10/11 SDK.
- Optional, for MSIX packaging only: the [`winapp` CLI](https://github.com/microsoft/WinAppCli).

### Build

```powershell
git clone https://github.com/sifatrhmn/keep-sticky-notes.git
cd keep-sticky-notes

# Run the test suite
cargo test

# Portable executable for the current machine
cargo build --release

# A specific architecture
cargo build --release --target aarch64-pc-windows-msvc
```

The executable is written to `target\<triple>\release\KeepStickyNotes.exe`.

To build, verify and package both architectures (portable `.exe` and MSIX) in one step:

```powershell
powershell -ExecutionPolicy Bypass -File tools\build_dist.ps1
```

The script generates a local self-signed certificate in `packaging\` on first use. That
certificate and its password file are private and are excluded from version control.

## Usage

1. Start the application. A tray icon appears and the Note Manager opens.
2. Press `Win+Alt+N`, or use the tray menu, to create a note. Notes work offline without
   an account.
3. To synchronise, open **Note Manager → Account**, enter your Google address and choose
   **Sign In with Google**. Sign in on Google's page.
4. Synchronisation begins in read-only mode. When you are satisfied that your notes have
   arrived correctly, choose **Enable Two-Way Sync**.
5. **Sign Out** removes the stored token. Local notes remain on the computer.

Notes and backups are stored in `%LOCALAPPDATA%\KeepStickyNotes\`. Set the environment
variable `KEEP_STICKY_DATA` to use a different folder.

### Known limitations

- Rich formatting created in Keep itself is neither displayed nor edited.
- Images are retained but not shown, and reminders are not synchronised.
- Labels can be assigned but not created.
- There is no push channel, so synchronisation is by polling.

## Folder structure

```
├── src/
│   ├── main.rs, lib.rs   Entry point and library root
│   ├── store.rs          SQLite cache of notes and window state
│   ├── keep.rs           Keep wire format, live backend and mock server
│   ├── sync.rs           Synchronisation engine and conflict rules
│   ├── auth.rs           Token exchange
│   ├── creds.rs          Windows Credential Manager storage
│   ├── format.rs         Formatting markers
│   └── ui/               Note windows, Note Manager, account, themes, About
├── tests/                Data-safety tests run against the mock server
├── res/                  Icon, manifest and resource script
├── packaging/            MSIX manifest and logo assets
├── tools/                Build, packaging and icon scripts
└── build.rs              Embeds the Windows resources
```

## Contributing

Contributions are welcome.

1. Open an issue to discuss a substantial change before starting work.
2. Fork the repository and create a branch for your change.
3. Run `cargo fmt`, `cargo clippy` and `cargo test` before submitting.
4. Open a pull request describing what changed and why.

Changes to synchronisation must keep the data-safety tests in `tests/sync_safety.rs`
passing, and new behaviour there should come with a test. Please never include tokens,
account details or note content in issues or pull requests.

## Disclaimer

This is an independent, free and open-source project. It is not affiliated with, endorsed
by or supported by Google LLC or Microsoft Corporation. "Google Keep" is a trademark of
Google LLC.

The synchronisation protocol is unofficial and undocumented. The stored token grants
broad access to the signed-in Google account and should be guarded like a password. The
software is provided "as is", without warranty of any kind; see the [licence](LICENSE).

## Credits

- The protocol implementation follows the public work of the
  [gkeepapi](https://github.com/kiwiz/gkeepapi) and
  [gpsoauth](https://github.com/simon-weber/gpsoauth) projects.
- Built with the [`windows`](https://github.com/microsoft/windows-rs),
  [`rusqlite`](https://github.com/rusqlite/rusqlite),
  [`ureq`](https://github.com/algesten/ureq) and
  [`webview2-com`](https://github.com/wravery/webview2-rs) crates.

## Licence

Released under the [MIT Licence](LICENSE). Copyright © 2026 Dr Sifat.
