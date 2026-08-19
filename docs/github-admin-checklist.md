# GitHub 仓库管理员待办

> 本清单记录无法通过仓库代码自动完成的 GitHub 平台配置。完成一项后勾选；证书、密码和其他敏感信息只能保存在 GitHub Secrets，绝不能提交到仓库、Issue、日志或截图中。

## 对外公开前

- [ ] 在准备对外公开时，将 GitHub 仓库可见性设为 **Public**，并确认提交历史、配置和文档中不含凭据、个人数据或其他不应公开的信息。
- [ ] 启用 **Private Vulnerability Reporting**，使安全研究者可以私下提交漏洞报告。
- [ ] 启用 **Secret Scanning**；若当前 GitHub 计划支持，也启用 Push Protection。
- [ ] 启用 **Dependabot alerts**。仓库已包含 Dependabot 更新配置，但告警开关需要管理员在 GitHub 平台启用。

## Windows 稳定版签名

- [ ] 在仓库的 Actions secrets 中配置 `WINDOWS_CERTIFICATE_BASE64`：用于签名的 PFX 证书的 base64 内容。
- [ ] 在仓库的 Actions secrets 中配置 `WINDOWS_CERTIFICATE_PASSWORD`：该 PFX 证书的密码。
- [ ] 确认证书私钥未保存在 Git、工作目录、共享目录或任何公开渠道中，并记录证书轮换与吊销责任人。

稳定标签（`vX.Y.Z`）缺少任一签名 secret 时，发布工作流会失败；这是一项有意的保护措施。带预发布后缀的标签（例如 `vX.Y.Z-rc.1`）可以生成未签名预发布，但 Release Notes 必须明确标注未签名状态。

## 首个稳定版发布后核验

- [ ] 从已审阅的提交创建并推送新 tag，例如 `v0.1.0`，以触发 Release 工作流。
- [ ] 确认 GitHub Release 包含 Windows NSIS 安装程序、`LICENSE`、`SHA256SUMS.txt`、两份第三方 notices 以及 Rust/npm 两份 SPDX SBOM。
- [ ] 验证稳定版安装程序的 SHA-256 和 Authenticode 签名；具体命令见[发布指南](release.md)。

上述事项均需要仓库管理员权限；完成后，源码、CI 和稳定版发布链路即具备完整的开源发布保障。
