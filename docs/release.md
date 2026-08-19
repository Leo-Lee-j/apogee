# 发布指南

本文定义 Apogee 官方发布的范围、验证步骤和产物要求。它是发布者的操作标准，不代表未列出平台已经受支持。

## 当前发布范围

| 项目 | 当前状态 |
| --- | --- |
| 源码许可证 | MIT，见根目录 `LICENSE` |
| 官方二进制目标 | Windows 10/11 x64 |
| 安装格式 | NSIS：`Apogee_<version>_x64-setup.exe` |
| WebView2 | 使用下载引导程序；目标机缺少 Runtime 时首次安装需要联网 |
| macOS | 架构与合规配置已考虑，但尚未构建、签名或作为官方产物发布 |
| Linux | 不在 v1.0 官方支持范围；不得把自行构建的文件标为官方版本 |

## 发布前准备

1. 确认工作树干净，版本在 `package.json`、根 `Cargo.toml`（workspace package）和 `src-tauri/tauri.conf.json` 中一致。
2. 更新 `CHANGELOG.md`，将已发布的改动从 `Unreleased` 移入带日期的版本段。
3. 从干净依赖状态运行以下检查：

   ```powershell
   pnpm install --frozen-lockfile
   pnpm audit --prod --audit-level=low
   pnpm typecheck
   pnpm test
   pnpm build
   pnpm check:contrast
   pnpm licenses:check

   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   cargo build -p apogee-desktop --locked
   pnpm test:e2e

   # 发布材料（生成文件保持 .gitignore）
   pnpm licenses:rust
   pnpm licenses:npm
   pnpm generate:sbom
   ```

4. 审阅 `git diff --check` 和依赖变更。任何新的许可证例外、RustSec 忽略、网络权限或 Tauri capability 都需要单独解释。
5. 使用形如 `v0.1.0`（稳定版）或 `v0.1.0-rc.1`（预发布）的已审阅 Git tag 触发发布工作流。不要通过移动或重用旧 tag 覆盖已发布产物。

## 工作流产物

标签发布工作流在干净的 Windows runner 上生成并上传以下内容到 GitHub Release：

- Windows x64 NSIS 安装程序；
- 项目 `LICENSE`；
- `SHA256SUMS.txt`，覆盖每个发布附件（除自身）的 SHA-256；
- `THIRD-PARTY-NOTICES.txt`，由 `cargo-about` 为 Rust 发行依赖生成的归属与许可证声明；
- `THIRD-PARTY-NOTICES-NPM.txt`，由 pnpm 生产依赖清单和包内许可证材料生成的 npm 归属声明；
- `apogee-rust.spdx.json`：Cargo workspace 解析依赖的 SPDX 2.3 源码/构建输入 SBOM；
- `apogee-npm.spdx.json`：pnpm production dependencies 的 SPDX 2.3 SBOM；
- 一个可回溯至该 workflow run 的 Actions artifact，以及包含签名状态的 Release Notes。

两类 notices 和两份 SBOM 都是归属与供应链材料，不是本项目许可证的替代品；MIT 正文始终以仓库和发布资产中的 `LICENSE` 为准。SBOM 描述锁定的源码/构建输入，`SHA256SUMS.txt` 才用于确认下载的具体安装程序。依赖的许可白名单、来源和已知公告例外由 `deny.toml` 管理，新增例外必须有可审阅的具体理由。

## 签名与稳定版门槛

稳定标签严格匹配 `v<major>.<minor>.<patch>`。这类发布必须配置以下 GitHub Actions secrets：

- `WINDOWS_CERTIFICATE_BASE64`：用于签署的 PFX 证书的 base64 内容；
- `WINDOWS_CERTIFICATE_PASSWORD`：该 PFX 的密码。

工作流会在临时目录还原证书、签署 Windows 安装程序，并在发布前用 Authenticode 验证。稳定标签缺少任一 secret 时必须失败，绝不能降级为未签名稳定版。含预发布后缀的 tag 可以在尚未配置证书时构建为未签名预发布；发布说明必须明确标注这一状态。

证书私钥不得存入 Git、Issue、日志、截图或本地共享目录。证书轮换、吊销或时间戳服务失败时，应停止稳定版发布、修复签名链后使用新版本 tag 重新发布，而不是替换既有附件。

## 下载者验证

下载官方 Release 后，至少执行以下检查：

```powershell
Get-FileHash .\Apogee_<version>_x64-setup.exe -Algorithm SHA256
Get-AuthenticodeSignature .\Apogee_<version>_x64-setup.exe
```

将第一条命令的哈希与同一 Release 的 `SHA256SUMS.txt` 对照。稳定版的第二条命令应显示有效签名；预发布若尚未签名，会在 Release Notes 中明确说明，用户应仅在理解风险后使用。

## 安全与回滚

- 在首个稳定版前，仓库管理员须启用 GitHub Private Vulnerability Reporting、Dependabot alerts 和 Secret Scanning；仓库文档不能代替这些平台开关。具体操作与首发核验项目见 [GitHub 仓库管理员待办](github-admin-checklist.md)。
- 发现高危漏洞时，先按 [安全政策](../SECURITY.md) 协调修复和披露，再发布新的不可变版本及完整校验材料。
- 已发现错误的发布不要悄悄替换附件。应标记或撤回受影响 Release、在说明中给出受影响版本和替代版本，并保留足够的审计线索。
- 代码签名或 SBOM 不能证明产品无漏洞；它们只帮助确认发布来源、内容和依赖构成。