# 发布指南

本文定义 Apogee 官方发布的范围、验证步骤和产物要求。它是发布者的操作标准，不代表未列出平台已经受支持。

## 最简单的发布方式

仓库公开后，无需额外 Token、Windows 证书或更新签名密钥。将已审阅代码提交并推送，确保版本号一致，再推送对应的版本 tag：

```powershell
# 示例：三处版本号均为 0.1.0，当前提交已包含发布工作流
git tag v0.1.0
git push origin v0.1.0
```

GitHub Actions 会自动检查、构建安装包、生成校验和等附件并创建 Release，不用手工上传。正式版与预发布都可以不签名；应用直接检查公开的 Releases 并下载更新。已发布的 tag 不要重用。以下是详细说明，证书和平台安全开关均不是额外的必配项。

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

## 可选 Windows 签名

默认无需配置证书，稳定标签 `v<major>.<minor>.<patch>` 和预发布标签都可以直接发布未签名安装包。Release Notes 会明确标注未签名状态；Windows 可能显示 SmartScreen 或发布者未知的提示。

仅在需要 Windows 代码签名时，成对配置以下 GitHub Actions secrets：

- `WINDOWS_CERTIFICATE_BASE64`：用于签署的 PFX 证书的 base64 内容；
- `WINDOWS_CERTIFICATE_PASSWORD`：该 PFX 的密码。

两个 secret 均未配置时正常跳过签名；只配置一个时会提示补全或移除不完整配置。配置齐全时，工作流在临时目录还原证书、签署安装程序并验证 Authenticode，签署或验证失败仍会阻止发布，不会静默改发未签名包。

证书私钥不得存入 Git、Issue、日志、截图或共享目录。启用签名后，证书或时间戳服务故障应修复后用新版本 tag 发布，不要替换既有附件。

## 下载者验证

下载官方 Release 后，至少执行以下检查：

```powershell
Get-FileHash .\Apogee_<version>_x64-setup.exe -Algorithm SHA256
# Release Notes 标注已签名时，再验证发布者签名
Get-AuthenticodeSignature .\Apogee_<version>_x64-setup.exe
```

将哈希与同一 Release 的 `SHA256SUMS.txt` 对照。若 Release Notes 标注已签名，第二条命令应显示有效签名；未签名包显示 `NotSigned` 属于预期，但哈希仍必须匹配。SHA-256 是完整性校验，不是独立的发布者签名认证。

## 安全与回滚

- 建议按需启用 GitHub Private Vulnerability Reporting、Dependabot alerts 和 Secret Scanning；这些平台开关不阻塞标签发布。见 [GitHub 发布与可选配置](github-admin-checklist.md)。
- 发现高危漏洞时，先与维护者私下协调修复和披露，再发布新的不可变版本及完整校验材料。
- 已发现错误的发布不要悄悄替换附件。应标记或撤回受影响 Release、在说明中给出受影响版本和替代版本，并保留足够的审计线索。
- 代码签名或 SBOM 不能证明产品无漏洞；它们只帮助确认发布来源、内容和依赖构成。
## 应用内检查更新与下载

- 启动页“软件更新”、“设置 → 关于 → 软件更新”及底部状态栏提供手动检查；当前版本读取桌面宿主版本，不硬编码在 UI。
- 更新源固定为 `Leo-Lee-j/apogee` 的 GitHub Releases。检查最近 100 个已发布 Release 的 `v<semver>` 标签，按语义版本比较而非按名称或发布时间排序；忽略草稿、非版本标签和构建元数据标签。默认稳定渠道，用户可包含预发布版。没有 Release 的空列表与网络失败分开显示。
- 普通 git tag 尚未发布 Release 或缺少安装资产时不能安装。最高版本 Release 必须包含唯一的 `Apogee_<version>_x64-setup.exe` 和逐行 `SHA256SUMS.txt`，且下载 URL 必须与本仓库、该标签、该文件名完全一致；不回退到缺少校验和的旧包。
- 更新 API 使用匿名访问，不读取请求 Auth、不发送工作区内容，也不自动读取 GitHub 登录凭据。仓库及 Release 必须允许公开下载。API 404（未公开或删除）、限流、超时和畸形元数据是错误，不会误报“已是最新版”。
- Rust 网络层仅允许 GitHub / 官方资产 CDN 的 HTTPS 重定向，始终验证 TLS；元数据上限 1 MiB、安装包上限 512 MiB、连接及读取超时、磁盘空间检查。不会沿用接口的“不验证 TLS”等设置。
- 安装包流式写入系统应用缓存目录的 `updates` 子目录，使用独占 `.part` 文件；校验字节数及 SHA-256 后才发布为 `.exe`。取消覆盖连接、首字节等待和读取；失败/正常取消清理本次临时文件。进程被强制终止或断电可能留下 `.part`，可在退出应用后手动清理此缓存目录。
- 同一应用只允许一个更新操作；下载进度按操作 ID 隔离。关闭设置页或切换工作区不取消下载。完整且有效的同版本缓存可复用；不清理用户选择的目录或 Cargo 缓存。
- 下载完成后，用户可“在文件夹中显示”或确认“打开安装包”。后台在打开前重新校验同一缓存文件；前端不能传入任意可执行路径。安装程序可能请求 UAC，应用不会自动退出或自动安装，请先保存请求并手动退出后完成安装。
- 这条更新链使用官方 GitHub Release 的 SHA-256 作为传输完整性校验，**不是 conduit 的独立签名 manifest 更新协议**，也不在应用内宣称已验证 Authenticode 发布者。Windows 代码签名为可选，稳定版和预发布包都可能未签名；以 Release Notes 的签名状态为准。
