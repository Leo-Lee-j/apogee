# 贡献指南

感谢你考虑为 Apogee 贡献。我们欢迎可复现的 bug 报告、文档修正、测试、性能测量和范围清晰的代码改动。请先阅读 [行为准则](CODE_OF_CONDUCT.md) 和 [安全政策](SECURITY.md)；安全漏洞不得通过公开 Issue 或 Pull Request 报告。

## 开始前

1. 先搜索已有 Issue 和 Pull Request，避免重复工作。
2. 较大的功能、跨层架构改动或新增依赖，请先开 Issue 说明问题、备选方案、性能与兼容性影响，等待维护者确认方向。
3. 不要在 Issue、日志、截图、测试夹具或提交中包含 API Token、Cookie、真实用户数据、内部主机名或其他凭据。
4. 当前受支持的发布目标是 Windows x64；欢迎改善跨平台可编译性，但不要把未验证的平台标注为已支持。

## 本地环境

请准备 Rust `1.89+`、pnpm `11.17.0`、Node.js、Windows 的 C++ Build Tools / Windows SDK，以及 WebView2 Runtime。安装依赖时使用锁文件：

```powershell
pnpm install --frozen-lockfile
```

常用开发命令：

```powershell
pnpm tauri dev
pnpm typecheck
pnpm test
pnpm build
pnpm check:contrast

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

桌面 E2E 会启动真实 Tauri/WebView2 窗口，并使用固定的本地开发端口；请在 Windows 交互式会话中运行：

```powershell
cargo build -p apogee-desktop --locked
pnpm test:e2e
```

`pnpm licenses:check` 运行 Rust 依赖的 `cargo deny check`。新增或升级依赖时，还应运行生产依赖审计并说明许可证、包体和供应链影响：

```powershell
pnpm audit --prod --audit-level=low
pnpm licenses:check
```

## 代码与设计约束

- 修改前阅读相关设计文档和相邻实现；`CLAUDE.md` 汇总了跨层、性能和安全约束。
- 前端不得绕开 `src/ipc/index.ts` 直接调用 Tauri；Rust 领域逻辑应优先位于不依赖 Tauri 的 `apogee-core`。
- 大响应体不能整体经 JSON IPC 传给前端，也不能在热路径创建无界缓存。
- 保持 Tauri 权限和 CSP 最小化；不得为方便调试而放开任意文件、shell 或网络访问。
- UI 颜色与状态优先使用现有 design tokens；改动颜色时运行 `pnpm check:contrast`。
- 不手工提交 `dist/`、`target/`、`node_modules/`、生成的 `THIRD-PARTY-NOTICES.txt`、用户工作区或性能测量中间产物。

## Pull Request 要求

每个 Pull Request 应当：

- 聚焦一个问题，说明动机、实现方式、风险和验证结果；关联相关 Issue（若有）。
- 保持提交和 diff 可审阅；不要混入无关格式化或生成文件。
- 为行为变更更新或新增合适的测试；不能自动化的步骤要写出可复现的手工验证方法。
- 对 UI 改动提供截图或简短录屏；对性能敏感改动说明测量结果或未测量的原因。
- 在请求审阅前运行与改动相关的门禁。CI 通过不自动代表设计、隐私、安全或平台兼容性已获批准。

维护者可能要求拆分改动、补充测试、调整文档或拒绝超出当前产品边界的提案。

## 贡献许可

除非你在贡献中明确另行说明并已获维护者书面同意，提交到本仓库的代码、文档和其他可版权保护材料均按仓库的 [MIT License](LICENSE) 许可。你声明自己有权提交这些内容，并且没有引入受第三方保密义务、未知来源或与项目分发策略不兼容的材料。