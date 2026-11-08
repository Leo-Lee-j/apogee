# Apogee

Apogee 是一个本地优先、启动即用的桌面 API 客户端。它以 Tauri、Rust 和 Vue 构建，目标是在保留日常 API 调试所需能力的同时，控制启动时间、内存占用和安装包体积。

> **项目状态：`0.1.0`。** 源码已按 [MIT](LICENSE) 公开；当前可验证、计划发布的桌面安装包范围为 Windows 10/11 x64。macOS 与 Linux 尚未提供受支持的发布物，请勿将规划文档中的平台目标视为已交付支持。

## 原则

- **本地优先**：工作区与集合保留在用户选择的本地目录；核心流程不要求账号。
- **可审计、可复现**：源码、锁文件、许可证门禁和发布校验信息均公开；开源不替代发布校验，正式发布仍应核对校验和、SBOM 以及签名（如有）。
- **性能是产品约束**：大响应体走 Rust 侧的流式存储和按需读取，避免把完整内容经 JSON IPC 送入前端。
- **数据可迁移**：集合使用面向 Git 的文本格式，并计划维护公开 Schema 与向后兼容承诺。

产品、交互与架构约束分别见 [产品设计](docs/01-产品设计文档.md)、[交互设计](docs/02-交互与界面设计.md) 和 [技术架构](docs/03-技术架构与数据模型.md)。

## 获取与安装

稳定版或预发布安装包将发布在 [GitHub Releases](https://github.com/Leo-Lee-j/apogee/releases)。当前 NSIS 配置采用 WebView2 下载引导程序：目标 Windows 设备若没有 WebView2 Runtime，首次安装需要联网。

应用内可从启动页“软件更新”、设置“关于”或底部状态栏检查 GitHub 版本标签，下载经过大小与 SHA-256 校验的安装包。默认只检查稳定版，支持预发布渠道和取消下载；安装需用户确认，应用不会自动退出。更新源须公开可访问，详见 [发布与更新说明](docs/release.md#应用内检查更新与下载)。

在没有可用发布物时，可按以下步骤从源码构建。开发环境需要：

- Windows 10/11、WebView2 Runtime，以及 Visual Studio C++ Build Tools / Windows SDK；
- Rust `1.89` 或更高版本，并安装 `x86_64-pc-windows-msvc` 目标；
- 与锁文件兼容的 Node.js，以及 pnpm `11.17.0`（仓库在 `packageManager` 字段中固定）。

```powershell
pnpm install --frozen-lockfile
pnpm typecheck
pnpm test
pnpm tauri dev
```

生产构建：

```powershell
pnpm build
pnpm tauri build --bundles nsis
```

Tauri 的生产打包仅执行 `pnpm build:nocheck`，所以不能以打包成功替代 TypeScript 检查；请始终单独运行 `pnpm typecheck` 或 `pnpm build`。

## 本地验证

提交前按改动范围运行相应门禁：

```powershell
pnpm typecheck
pnpm test
pnpm build
pnpm check:contrast
pnpm licenses:check

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

真实桌面 E2E 只支持 Windows，并会启动本地 Vite、Tauri 和隔离的 WebView2 数据目录：

```powershell
cargo build -p apogee-desktop --locked
pnpm test:e2e
```

推送版本 tag 即可自动打包并发布 GitHub Release，无需配置 Windows 证书或额外 Token。完整发布检查、产物清单、SBOM、归属声明和可选签名说明见 [发布指南](docs/release.md)。

## 参与贡献

欢迎 bug 报告、文档改进、测试补充和聚焦的功能改动。开始前请阅读：

- [贡献指南](CONTRIBUTING.md)
- [行为准则](CODE_OF_CONDUCT.md)
- [安全政策](SECURITY.md)（安全问题不要公开提交 Issue）
- [变更记录](CHANGELOG.md)

## 许可证与第三方组件

Apogee 源码以 [MIT License](LICENSE) 发布。二进制发布物还会附带 Rust 与 npm 两类第三方归属声明（`THIRD-PARTY-NOTICES.txt`、`THIRD-PARTY-NOTICES-NPM.txt`）及对应的 SPDX SBOM；请从官方 Release 附件取得并核对。项目自身的 MIT 授权不会改变任何第三方组件的许可证义务。
