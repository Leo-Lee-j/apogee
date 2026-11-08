<p align="center">
  <img src="src-tauri/icons/128x128.png" alt="Apogee logo" width="128" height="128">
</p>

<h1 align="center">Apogee</h1>

<p align="center">
  English · <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  A local-first desktop API client, ready when you are.<br>
  Requests, collections, environments, and responses in one lightweight workspace.
</p>

<p align="center">
  <a href="https://github.com/Leo-Lee-j/apogee/actions/workflows/ci.yml"><img src="https://github.com/Leo-Lee-j/apogee/actions/workflows/ci.yml/badge.svg" alt="Windows CI"></a>
  <img src="https://img.shields.io/badge/platform-Windows_x64-0078D4" alt="Windows x64">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-20cbb5" alt="MIT License"></a>
</p>

<p align="center">
  <a href="https://github.com/Leo-Lee-j/apogee/releases"><strong>Download for Windows</strong></a> ·
  <a href="CHANGELOG.md">Changelog</a> ·
  <a href="https://github.com/Leo-Lee-j/apogee/issues">Report an issue</a>
</p>

Apogee is built for everyday API testing: open a local workspace, organize endpoints, configure environments, send requests, and inspect responses—no Apogee account required. Built with **Tauri 2, Rust, and Vue 3**, it focuses on fast startup, low memory usage, and local data management.

The project is under active development and currently targets **Windows 10/11 x64**. Official macOS and Linux packages are not available yet; roadmap items are not shipped features.

## Features

- **Organize APIs.** Manage requests with workspaces, collections, and folders, with environment variables and inherited settings. Collections are stored as text files for easy backups and version control with Git.
- **Build requests.** Edit query and path parameters and headers. Use JSON, forms, files, binary data, or GraphQL request bodies, with bulk parameter pasting and editing.
- **Configure authentication.** Use Basic, Bearer, API Key, OAuth 1.0, OAuth 2.0, or JWT authentication, or inherit it from a collection or folder. OAuth 2.0 supports token acquisition, refresh, and authorization code flows with PKCE.
- **Inspect responses.** View status codes, headers, timing, and response content, and save responses to files. Rust streams large response bodies to local files and reads them on demand, rather than sending the entire body to the frontend.
- **Bring your existing APIs.** Paste cURL commands to import requests, import Postman collections and environments, export Postman collections, or preview and import YApi projects.
- **Make it your own.** Customize themes, fonts, and keyboard shortcuts, use request history and the command palette, and check for and download updates within the app.

## Getting started

### Installation

1. Open [GitHub Releases](https://github.com/Leo-Lee-j/apogee/releases), download `Apogee_<version>_x64-setup.exe`, and install it.
2. Launch Apogee and create a workspace or open an existing local workspace.
3. Create or import a request, enter the endpoint URL, select an environment, and send it.

Available installers are listed on the release page. If no installer is available yet, follow the source instructions below. If **WebView2 Runtime** is missing on the target device, the installer downloads it over the network.

### Data and updates

Workspaces, collections, and environments are stored in a local directory you choose. Requests connect to the endpoints you configure, and update checks connect to GitHub; local storage does not mean every feature works offline. Remove tokens, passwords, and other private information before sharing collections, logs, or screenshots.

Check for updates from the startup page's update section, **Settings → About**, or the bottom status bar. By default, only stable releases are checked; you can also include prereleases. Downloads can be canceled and are verified against their size and SHA-256 checksum. The installer opens only after your confirmation, and the app does not quit automatically. Windows code signing is optional, so unsigned installers may trigger system security warnings.

## Running from source

You need **Windows x64**, WebView2 Runtime, Visual Studio C++ Build Tools / Windows SDK, and:

- Rust **1.89 or newer** with the MSVC toolchain;
- Node.js **24**;
- pnpm **11.17.0**, matching the version in `package.json`.

```powershell
pnpm install --frozen-lockfile
pnpm tauri dev
```

Build a Windows installer:

```powershell
pnpm build
pnpm tauri build --bundles nsis
```

Run the checks relevant to your changes:

```powershell
pnpm typecheck
pnpm test
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Version tags trigger GitHub Actions to generate installers and verification files. No additional token or mandatory Windows signing certificate is required. See the [release guide](docs/release.md) for details.

## Documentation

The following documents are currently available in Chinese.

| Document | Contents |
| --- | --- |
| [Changelog](CHANGELOG.md) | Implemented changes and version history |
| [Product design](docs/01-产品设计文档.md) | Product goals, supported scope, and roadmap |
| [Interaction design](docs/02-交互与界面设计.md) | Interface structure and user flows |
| [Technical architecture](docs/03-技术架构与数据模型.md) | Data model, layers, and workspace lifecycle |
| [Release guide](docs/release.md) | Builds, release assets, verification, and in-app updates |

## License

Apogee is open source under the [MIT License](LICENSE). Third-party components retain their respective licenses. Official release assets include Rust / npm attribution notices and SPDX SBOMs.
