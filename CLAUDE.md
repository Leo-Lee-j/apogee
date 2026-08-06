# CLAUDE.md

本文件为在本仓库中工作的 Claude Code 提供项目级上下文。修改代码前先阅读相关设计文档和相邻实现；项目中的性能、安全与数据边界是硬约束，不是建议。

## 项目概览

Apogee 是一个本地优先、启动即用的桌面 API 客户端，目标是在保留 Postman 核心能力的同时显著降低启动时间、内存和安装包体积。

- 桌面容器：Tauri 2
- 前端：Vue 3.5、TypeScript、Vite、Pinia、Naive UI、CodeMirror 6
- 后端：Rust 2021、Tokio、Reqwest + rustls
- 包管理器：pnpm 11
- Rust 最低版本：1.85
- 当前主要平台：Windows；macOS 为 v1.0 目标，Linux 不在 v1.0 范围

权威设计文档：

- `docs/01-产品设计文档.md`：产品边界、性能预算、反目标
- `docs/02-交互与界面设计.md`：交互、视觉和错误展示规则
- `docs/03-技术架构与数据模型.md`：分层、IPC、响应体和持久化约束
- `docs/apogee-color-system.md`：颜色系统

文档与代码冲突时，先确认代码是否是已经落地的新决策；不要静默改写架构约束。

## 常用命令

```powershell
# 安装前端依赖
pnpm install

# 仅启动浏览器前端；Tauri IPC 在此模式下不可用
pnpm dev

# 启动完整桌面应用
pnpm tauri dev

# 前端类型检查与生产构建
pnpm typecheck
pnpm build

# Rust 格式、检查和测试
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# 许可与视觉门禁
pnpm licenses:check
pnpm check:contrast

# 性能/体积 Spike，耗时较长，按改动风险选择执行
pnpm spike:s1
pnpm spike:s3
pnpm spike:s5
```

Tauri 的 `beforeBuildCommand` 使用 `pnpm build:nocheck`，因此不能把桌面构建成功当作 TypeScript 检查通过；提交前应单独运行 `pnpm typecheck` 或 `pnpm build`。

仓库当前没有前端单元测试框架。前端改动至少运行类型检查和构建；Rust 领域逻辑应补充相邻模块的 `#[cfg(test)]` 单元测试。

## 代码结构

- `src/`：Vue 前端
  - `components/`：布局、请求、响应、侧边栏和基础 UI 组件
  - `views/`：设置、历史、命令面板等延迟加载视图
  - `stores/`：Pinia 状态
  - `ipc/index.ts`：前端与 Rust 通信的唯一出口及共享 IPC 类型
  - `shell/`：首屏应用骨架；启动热路径
  - `styles/tokens.css`：全局设计令牌
- `src-tauri/`：Tauri 宿主、命令注册、自定义协议、应用状态和工作区适配
- `crates/apogee-core/`：不依赖 Tauri 的领域核心；HTTP、变量、cURL、响应索引和工作区模型
- `scripts/`：Windows 性能量测、体积报告、图标和对比度检查
- `spikes/`：技术验证代码与历史实测结果，不等同于产品功能

## 架构硬约束

1. 任何 `O(响应体大小)` 或 `O(集合规模)` 的计算都放在 Rust 侧。前端不持有完整大响应、不递归构造整棵 JSON 树。
2. Vue 组件和 store 不得直接导入 Tauri `invoke`。所有命令、事件、错误归一化和协议 URL 都经过 `src/ipc/index.ts`。
3. 大响应体不得通过 JSON IPC 返回。Rust 必须流式写入磁盘 spool，前端通过 `apogee://` 自定义协议和 Range 请求按需读取。
4. `crates/apogee-core` 不得依赖 Tauri。Tauri 命令保持薄，只做参数校验、状态协调和领域层转发，以便未来 CLI 复用核心逻辑。
5. HTTP 客户端必须复用连接池；不要为每个请求创建新 `reqwest::Client`。请求取消必须覆盖 connect、TTFB 和下载阶段。
6. 启动路径只挂载可立即发送请求所需的 shell。CodeMirror、设置、历史、命令面板等继续动态加载，不要把工作区扫描、数据库、文件监听或网络请求塞进首屏路径。
7. 响应体、索引和标签状态必须有界。不要引入无上限缓存；标签变冷时应释放大对象，响应索引遵守 LRU 策略。
8. 保持 Tauri 能力最小化。不要随意启用 `protocol-asset`、额外 capability、宽泛 CSP 或任意文件系统访问。

## 跨层同步点

以下改动必须成组检查：

- 自定义协议名：`src-tauri/src/lib.rs`、`src-tauri/tauri.conf.json`、`src/ipc/index.ts`
- S1 就绪文件环境变量 `APOGEE_S1_READY_FILE`：Rust 启动逻辑与 `scripts/measure-s1-windows.ps1`
- IPC 类型/字段：Rust serde 类型、Tauri 命令参数、`src/ipc/index.ts` 和所有调用方
- 工作区文件格式：`crates/apogee-core` 模型、读写实现、格式版本和兼容性测试
- 应用名、bundle identifier 或数据目录：配置、协议、量测脚本和已有用户数据迁移策略

## 前端约定

- 使用 Vue Composition API、`<script setup lang="ts">` 和严格 TypeScript。
- 使用 `@/` 路径别名；应用 shell 通过 `@app-shell` 选择，避免破坏 Vite 的 S5 变体量测。
- 高频热路径优先使用本地轻量组件和 CSS；Naive UI 用于设置、弹窗等非热路径，并保持按需引入。
- 颜色、间距和状态样式优先复用 `src/styles/tokens.css`；不要另建一套平行设计令牌。
- 大对象使用浅响应或非响应结构，避免对大型树或响应数据做深度 `reactive`。
- 错误必须进入统一 `AppError` 展示，包含可操作的 `hints`；不要只写到控制台或吞掉异常。
- 请求事件可能早于 `send_request` 句柄返回，也可能在取消后迟到。修改 `src/stores/request.ts` 时必须保留按请求 id 收敛和 orphan 事件处理。

## Rust 约定

- 领域错误使用统一的 `AppError`、`ErrorKind` 和 `Hint`，不要把内部错误字符串直接泄露给 UI。
- 大文件与响应体使用流式 I/O；禁止用 `response.bytes().await` 等方式整体读入内存。
- 公共行为尽量落在 `apogee-core` 的纯函数或可测试服务中，Tauri 层只负责适配。
- 新依赖必须兼容闭源分发许可。禁止 GPL、AGPL、SSPL；新增依赖后运行 `pnpm licenses:check` 并评估包体影响。
- 保持根 `Cargo.toml` 的 release 体积配置及 reqwest/rustls feature 选择。不要引入 OpenSSL、默认 TLS 或需要 NASM/CMake 的 aws-lc-rs 路径。

## 性能与验证

性能是产品规格。尤其关注：冷启动、首屏 chunk、空闲内存、50 标签内存、10MB/100MB 响应、安装包和前端 gzip 体积。

- 修改启动、依赖、Vite 分包或 Tauri 配置：运行构建，并考虑 S1/S5。
- 修改响应存储、JSON 索引、标签生命周期：运行 Rust 测试，并考虑 S2/S3 相关量测。
- 修改 UI 颜色或状态：运行 `pnpm check:contrast`。
- 修改依赖：运行类型检查、Rust 测试、`pnpm licenses:check`，并说明体积影响。

不要手工修改 `dist/`、`target/`、`spikes/results/*.json` 或历史 `*-report.md`。这些是构建产物或实测记录；需要更新时应运行对应脚本生成。

## 工作方式

- 改动保持小而聚焦，优先沿用相邻代码的模式。
- 先定位权威数据流和所有调用方，再修改共享类型或跨层契约。
- 不做与任务无关的重构，不覆盖工作区中已有的用户改动。
- 注释解释竞态、性能边界和反直觉决策，不重复代码字面含义。
- 完成后报告实际运行的检查；未运行的长耗时或平台相关检查要明确说明。
