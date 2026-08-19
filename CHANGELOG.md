# 变更记录

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 的结构，并以 [Semantic Versioning](https://semver.org/lang/zh-CN/) 管理发布版本。未发布的工作先记录在 `Unreleased`；只有实际发布的标签才应获得发布日期。

## [Unreleased]

### Added

- 以 MIT 许可证公开 Apogee 源码。
- 贡献、安全、社区行为与发布流程文档。
- 面向 Windows 的 CI、依赖更新与发布自动化（发布前仍需由仓库管理员配置 GitHub 安全与签名设置）。

### Security

- 依赖锁定、许可证/来源/漏洞门禁和发布校验流程。
- 真实 Tauri/WebView2 工作区生命周期 E2E 覆盖。

## 发布说明约定

每个已发布版本应至少列出：

- `Added`：新功能或新平台支持；
- `Changed`：行为、性能或兼容性改变；
- `Deprecated` / `Removed`：弃用或移除内容；
- `Fixed`：缺陷修复；
- `Security`：安全修复（不得在修复可用前泄露可利用细节）。

发布标签、安装包名称、校验和、SBOM 和签名状态以 GitHub Release 为准；具体流程见 [docs/release.md](docs/release.md)。