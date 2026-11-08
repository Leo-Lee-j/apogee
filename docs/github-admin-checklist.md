# GitHub 发布与可选配置

## 直接发布

仓库设为 Public 后，无需额外 Token、Windows 证书或更新签名密钥：

1. 提交并推送已审阅代码，确保当前提交包含 `.github/workflows/release.yml`。
2. 确认 `package.json`、根 `Cargo.toml` 和 `src-tauri/tauri.conf.json` 的版本号一致。
3. 创建并推送匹配的版本 tag（如 `v0.1.0`）。Actions 自动构建并上传安装包、校验和、许可证、notices 和 SBOM，生成 GitHub Release。

工作流使用 GitHub 自带的 `github.token`，已声明 `contents: write`，不需要配置个人访问令牌。应用匿名访问公开 Release，即可检查和下载更新。流程与验证命令见[发布指南](release.md)。

## 可选配置，不是发布前提

- **Windows 代码签名**：需要时成对配置 `WINDOWS_CERTIFICATE_BASE64` 和 `WINDOWS_CERTIFICATE_PASSWORD`。均不配置即可发布未签名稳定版或预发布；只配一个会报错。Release Notes 会注明签名状态，未签名包可能出现 Windows 安全提示。
- **GitHub 安全功能**：建议启用 Private Vulnerability Reporting、Dependabot alerts、Secret Scanning，以及可用的 Push Protection。无需为此延迟发布。

证书、密码和其他凭据绝不能提交到仓库、Issue、日志或截图中；公开源码和提交历史也应确认不包含敏感信息。

## 发布后核验

确认 Release 附件完整，安装包 SHA-256 与同一 Release 的 `SHA256SUMS.txt` 一致。仅在 Release Notes 标注已签名时检查 Authenticode 签名。首个 Release 发布后，再从旧版本应用实际检查、下载并验证安装包；没有 Release 时无法完成在线下载验收。
