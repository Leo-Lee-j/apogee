<p align="center">
  <img src="src-tauri/icons/128x128.png" alt="Apogee logo" width="128" height="128">
</p>

<h1 align="center">Apogee</h1>

<p align="center">
  本地优先、启动即用的桌面 API 客户端。<br>
  请求、集合、环境和响应，在一个轻量工作区里完成。
</p>

<p align="center">
  <a href="https://github.com/Leo-Lee-j/apogee/actions/workflows/ci.yml"><img src="https://github.com/Leo-Lee-j/apogee/actions/workflows/ci.yml/badge.svg" alt="Windows CI"></a>
  <img src="https://img.shields.io/badge/platform-Windows_x64-0078D4" alt="Windows x64">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-20cbb5" alt="MIT License"></a>
</p>

<p align="center">
  <a href="https://github.com/Leo-Lee-j/apogee/releases"><strong>下载 Windows 安装包</strong></a> ·
  <a href="CHANGELOG.md">更新记录</a> ·
  <a href="https://github.com/Leo-Lee-j/apogee/issues">反馈问题</a>
</p>

Apogee 面向日常 API 调试：打开本地工作区，组织接口、配置环境、发送请求并查看响应，不需要注册 Apogee 账号。它使用 **Tauri 2、Rust 和 Vue 3** 构建，注重启动速度、内存占用和本地数据管理。

项目仍在持续开发，目前面向 **Windows 10/11 x64**。macOS 与 Linux 暂无官方安装包；规划中的功能不代表已经交付。

## 主要功能

- **组织接口。** 使用工作区、集合和文件夹管理请求，配置环境变量及继承设置；集合以文本文件保存，便于备份和纳入 Git。
- **调试请求。** 编辑 Query、Path 参数和 Header，支持 JSON、表单、文件、二进制与 GraphQL 请求体，以及参数批量粘贴和编辑。
- **配置认证。** 支持 Basic、Bearer、API Key、OAuth 1.0、OAuth 2.0 和 JWT；可从集合或文件夹继承认证，OAuth 2.0 提供令牌获取、刷新及授权码 + PKCE 流程。
- **查看响应。** 查看状态、Header、耗时和响应内容，保存响应文件；大响应体由 Rust 流式写入本地文件，按需读取，避免整体传入前端。
- **迁移现有接口。** 粘贴 cURL 导入请求，导入 Postman 集合与环境、导出 Postman 集合，或从 YApi 预览并导入项目。
- **保持顺手。** 自定义主题、字体和快捷键，使用请求历史与命令面板，并从应用内检查、下载更新。

## 开始使用

### 安装

1. 打开 [GitHub Releases](https://github.com/Leo-Lee-j/apogee/releases)，下载 `Apogee_<version>_x64-setup.exe` 并安装。
2. 启动 Apogee，创建工作区或打开已有的本地工作区。
3. 新建或导入请求，填写接口地址，选择环境并发送。

安装包以发布页实际提供的附件为准；若尚无可用安装包，可以按下方说明从源码运行。目标设备缺少 **WebView2 Runtime** 时，安装程序会联网下载。

### 数据与更新

工作区、集合和环境保存在用户选择的本地目录。请求会访问你配置的接口，更新检查会访问 GitHub；本地存储不意味着所有功能都能离线使用。分享集合、日志或截图前，请移除 Token、密码和其他私密信息。

在启动页“软件更新”、**设置 → 关于**或底部状态栏检查版本。默认只检查稳定版，也可包含预发布版；下载支持取消，并校验大小与 SHA-256。安装程序需用户确认后打开，应用不会自动退出。Windows 代码签名为可选，未签名安装包可能出现系统安全提示。

## 从源码运行

需要 **Windows x64**、WebView2 Runtime、Visual Studio C++ Build Tools / Windows SDK，以及：

- Rust **1.89 或更新版本**（MSVC 工具链）；
- Node.js **24**；
- pnpm **11.17.0**（与 `package.json` 中的版本一致）。

```powershell
pnpm install --frozen-lockfile
pnpm tauri dev
```

构建 Windows 安装包：

```powershell
pnpm build
pnpm tauri build --bundles nsis
```

按改动范围验证代码：

```powershell
pnpm typecheck
pnpm test
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

发布使用版本 tag 触发 GitHub Actions，自动生成安装包和校验材料，无需额外 Token 或必配的 Windows 证书。详细步骤见[发布指南](docs/release.md)。

## 文档

| 文档 | 内容 |
| --- | --- |
| [更新记录](CHANGELOG.md) | 已实现的改动与版本记录 |
| [产品设计](docs/01-产品设计文档.md) | 产品定位、支持范围与路线图 |
| [交互设计](docs/02-交互与界面设计.md) | 界面组织与操作流程 |
| [技术架构](docs/03-技术架构与数据模型.md) | 数据模型、分层与工作区生命周期 |
| [发布指南](docs/release.md) | 构建、发布附件、校验与应用内更新 |

## 许可证

Apogee 以 [MIT License](LICENSE) 开源。第三方组件遵循各自的许可证；官方发布附件包含 Rust / npm 归属声明及 SPDX SBOM。
