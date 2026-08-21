# Rust 工程可靠性优化实施方案

> 状态：实施中（已落地 session-bound response runtime 与 P0 默认脱敏子切片；完整 P0 尚未完成）
>
> 初稿：2026-08-20；最近更新：2026-08-21
>
> 范围：`crates/apogee-core`、`src-tauri` 与 Rust 侧的 IPC、工作区、响应缓存、导入和自动化链路。
>
> 关联文档：[01-产品设计文档](./01-产品设计文档.md) · [03-技术架构与数据模型](./03-技术架构与数据模型.md) · [04-工作区完善规划](./04-工作区完善规划.md)
>
> 已接受决策：[ADR-0001：P0 运行态可靠性与数据安全基线](./adr/0001-p0-runtime-safety-baseline.md)

## 实施状态快照（2026-08-21）

本轮已完成 **P0 的 session-scoped response runtime、局部单文件安全发布与默认脱敏子切片**；这仍不是完整的运行态/数据安全基线。下文 §1.1 的审查数据是 2026-08-20 的初始发现；以下内容为当前实现和继续工作的权威进度记录。

### 已落地

- 响应访问改为绑定 `workspaceId + sessionId`：`ResponseAccess` / `ResponseScope` 覆盖普通 response IPC、协议读取、摘要、Range、树、索引、请求事件和历史持久化，前端同时丢弃已切换 session 的迟到事件。
- `RequestLease` 在同一 session 临界区内固定取消句柄、response access 与 `Arc<ResponseRuntime>`；旧请求在 A → B 切换后最多收尾自己的 A runtime，不能向 B 发布 metadata、索引或终结事件。
- 每个 session 使用 `<runtime>/responses/<sessionId>` 私有 spool。`open_workspace` / `close_workspace` 先切换 active runtime 并取消旧任务，最后一个旧 runtime lease 释放时再删除旧目录；旧任务不会删除或写入新 session 数据。
- history body 恢复、Synthetic 生成和 JSON 索引采用“短锁捕获 runtime → 锁外 I/O/计算 → 发布前重新验证”的路径；Synthetic 通过 `Arc` 身份防止 open/close ABA。
- history body 导入使用唯一 `.part` 文件、flush 后以同目录 hard link 作为不可覆盖的发布点；已存在的 body 仅在与 history source 长度匹配时作为幂等恢复复用。若复制后的发布窗口发生竞争，只有明确的 `AlreadyExists` 才会复用匹配 body，其他发布/I/O 失败会原样返回；成功发布或复用后的临时删除失败同样可见为错误，`Spool::remove` 会回收该 response 遗留的 `.part`。history restore 不再以 `exists()` 裸短路。同一 response 的并发发布、已存在 body 的匹配复用、长度不匹配拒绝、`AlreadyExists` 竞争、非冲突失败传播、临时清理失败和 remove 回收均有回归测试。response index 和 metadata 会在 session 切换时清理。
- Windows 单文件发布已移除“先删除旧文件再 rename”的空窗，应用设置、工作区状态和 YAML 写入在替换失败时保留旧可读文件；CI 也补充了声明 MSRV 与 stable 的 Rust 质量门禁。
- **P0 默认脱敏子切片已落地，并已按安全复审补强。** `apogee-core::public` 定义了 `PublicRequestPreview`、`PublicActualRequest`、`PublicResponseSummary`、`PublicError`/`PublicHint` 与安全的键值、设置投影；完整 body、auth、代理 URL、文件路径、错误 detail 和不安全提示不会进入这些 DTO。URL 会移除 userinfo/fragment，并依据敏感来源遮蔽中性名称的 query、header、Cookie、Bearer/Basic/API key、`ValueKind::Secret` 与 `ValueKind::File`；路径值同时遮蔽原文及 URL 编码形式（百分号十六进制大小写无关）。
- `ActualRequest` 仅在执行内存中保留且不会 serde 化的敏感来源元数据；`snapshot_request` 从解析后的 request、鉴权和 API key 采集它们，公开投影可遮蔽中性字段名（例如 `id`、`q`、`X-My-Key`）以及 URL 编码后的路径值（例如 `a b` → `a%20b`）。配置的自定义 API-key Header 即使被同名手写 Header 覆盖，仍被视为敏感来源；实际请求和 history preview 的同名行都会遮蔽覆盖值。
- history YAML、新旧 history 读取、`request:done` / `request:error`、`get_response_meta`、`index_response` 和相关 Spike 响应均只使用公开 DTO。旧 history 继续只读兼容，读取时会在原始 `Secret`/`File` 标记尚可用时补偿净化实际请求；旧 `auth` 则仅解析无值的 `type`/`key`/`location` 脱敏来源，因此自定义 API-key Header/Query 也会在 legacy preview 行、失败记录和嵌入 response 的 actual request 中被遮蔽，且旧 YAML 不会自动重写或删除。
- 前端 history IPC 先构造公开 preview，并根据中性 `Secret` query 名、大小写不同的编码 path 值和自定义 query/header API-key 名净化 URL 与 preview 行；历史记录以无 body/auth 的安全 preview 重开。`historyPreview` 在请求执行入口被显式拒绝，快捷键或未来调用方也不能发送该标签；响应面板不显示 actual request body/body 参数或 error detail。普通请求终结日志和 panic hook 只记录安全摘要，不格式化原始执行快照或错误 detail。用户工作区请求 YAML 仍是原始 source of truth，未被本切片改写。

### 已验证

本切片实施期间已通过以下针对性回归：

| 检查 | 结果 |
| --- | --- |
| `cargo +stable test -p apogee-core --lib --locked` | 通过（72 项；含中性 query 来源、大小写不同的编码 path、覆盖式 API-key Header 与公开 DTO 回归） |
| `cargo +stable test -p apogee-desktop legacy_history_is_read_compatibly_but_sanitized_before_output --lib --locked` | 通过（旧 YAML 的中性命名 Secret 补偿净化） |
| `cargo +stable test -p apogee-desktop legacy_custom_api_key_sources_redact_top_level_and_embedded_actual_requests --lib --locked` | 通过（legacy 自定义 API-key Header/Query 的失败与成功 actual request 脱敏） |
| `cargo +stable test -p apogee-desktop workspace::tests::history --lib --locked` | 通过（公开 history YAML 持久化和读取） |
| `get_diagnostics`（脱敏相关 Rust 文件） | 无诊断 |

本切片已按受影响范围完成完整门禁且全部通过：`cargo +stable fmt --all -- --check`、`cargo +stable clippy --workspace --all-targets --locked -- -D warnings`、`cargo +stable test --workspace --locked`（`apogee-core` 72、`apogee-desktop` 38）、`pnpm typecheck`、`pnpm exec vitest run --reporter=verbose`（5 个文件、16 项测试）与 `git diff --check`。前端 IPC preview 与 history preview 不可执行回归均包含在该门禁内。

### 下一轮优先级与已知边界

1. **完成 response runtime 的整体收敛。** 目前是 `ResponseRuntime`，而非目标中的完整 `WorkspaceRuntime`；repository、response registry、in-flight、mutation gate 和计划缓存仍未统一为同一 owning runtime。普通协议 URL 仍是 session 归属校验，尚不是随机、短期 response capability。
2. **完成阶段 2 的持久化协议。** 尚无跨进程单写者锁、`MutationGate`、staging/journal/tombstone/recovery；成功 history body 复制也仍需拆为锁外复制、重新验证、发布的两阶段流程。
3. **完成阶段 3 的资源与协议边界。** 尚无 500 MiB runtime 记账、构建前 JSON 预算/single-flight、统一 `ResponseRegistry`、完整 Range/MIME/受限协议 I/O 改造。
4. **增强 body 完整性校验。** 本轮已验证已有 body 的长度并拒绝错误发布；同长度损坏尚不可区分。若威胁模型或恢复诊断需要，应引入可选 hash/长度元数据并权衡额外 I/O，而不是重新放开 `exists()` 裸短路。
5. **设计受控 reveal。** 本切片不提供 reveal；若未来确有排障需求，必须先定义显式同意、短时 capability、工作区绑定、审计和过期语义，不能把公开 DTO 重新变成默认明文通道。

### 已完成：默认脱敏最小切片

本切片实现“**默认公开数据不得含执行秘密**”，且不迁移或修改用户工作区中保存的请求定义：

1. 在 Rust 侧新增公开 DTO 和统一净化函数，将内部 `RequestSpec`、`ActualRequest`、`ResponseSummary` 与 `AppError` 投影为可显示/可持久化视图；默认遮蔽敏感 header/query/URL userinfo、Cookie、Bearer/Basic/API key、密码、`ValueKind::Secret`/`File`，并移除完整 body、auth、代理 URL、文件路径和 error detail。
2. `ActualRequest` 增加仅内存、不可 serde 的敏感来源元数据；`snapshot_request` 从已解析 request 和鉴权配置收集路径、query、header 的来源。公开 URL 同时替换原始与 `urlencoding_path` 形式，并以百分号十六进制大小写无关的方式匹配，因此中性 `id` 的 `a b`（`a%20b`）和 `é`（`%c3%a9`）都不会绕过脱敏；自定义 API-key Header 即使被同名手写 Header 覆盖，实际请求和 history preview 也保持敏感。
3. history 写入/读取和失败记录只使用公开 DTO；旧 YAML 兼容读取后立即重净化，不自动 rewrite/delete。读取兼容层会利用仍可解析的原始 `Secret`/`File` 行，以及仅保留 `type`/`key`/`location` 的无值 legacy auth 脱敏来源，补偿净化旧 `actualRequest` 和嵌入 response 的 actual request。
4. 请求终结事件、response metadata/index 与前端 IPC 均使用公开 DTO；history 打开不会恢复可执行 body/auth，且 tab-level `historyPreview` 会在 `useRequest().send()` 的执行入口被拒绝。响应 UI 不再暴露 actual body/body 参数/error detail；普通日志只保留安全 code/kind/摘要。
5. 回归覆盖 body/auth 变体、Cookie/Authorization、API key、Secret/File、中性 `id`/`q`/`X-My-Key`、大小写不同的编码路径、API-key Header 覆盖及其 history preview、旧 history YAML 的 custom API-key Header/Query、不可执行 history preview、非敏感摘要以及前端 history preview。明确不包含 reveal、旧数据自动清理、工作区请求 YAML 改写或新的第三方依赖。

本切片涉及 `crates/apogee-core/src/{model,http,public}.rs`、`src-tauri/src/{lib,workspace,state}.rs`、`src/ipc/index.ts`、`src/stores/request.ts` 与 history/response 视图组件。应把完整 response body 的本地 spool 保留与其公开 DTO 解耦；资源预算、hash 完整性、随机 response capability、跨进程写者锁和多文件事务继续留在后续切片。

本文把 Rust 全局审查的结论转为可实施的工程计划。它不改变 Apogee 的产品定位：工作区 YAML 仍是用户数据真相来源，应用仍以本地优先、性能优先和可迁移为原则；优化目标是让当前实现真正满足已有文档承诺的工作区隔离、可靠写入、响应资源控制和敏感信息边界。

---

## 1. 背景、结论与边界

### 1.1 当前基线

当前 Cargo workspace 包含三个 crate：

- `apogee-core`：请求、变量、响应、导入、自动化和工作区数据契约；不依赖 Tauri。
- `apogee-desktop`：Tauri 命令、应用状态、YAML 工作区仓储、运行态、协议和平台能力。
- `s6-postman-schema`：Postman schema 验证 Spike。

本轮只读验证结果：

| 检查 | 结果 | 说明 |
| --- | --- | --- |
| `cargo fmt --all -- --check` | 通过 | Rust 格式一致 |
| `cargo check --workspace --all-targets --locked` | 通过 | workspace 可编译 |
| `cargo test --workspace --locked` | 通过 | core 54、desktop 16，共 70 个单元测试 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 未通过 | 本机 Rust/Clippy 1.95 发现 3 个 `too_many_arguments`：`record_failed_history`、`history_record`、`history_record_error` |

测试通过说明现有正常路径有基础保障；它**不**证明并发写、进程中断、磁盘失败、跨工作区切换、超大响应或秘密泄露边界正确。优化工作必须以这些缺失场景为验收中心。

### 1.2 本方案解决的问题

优先解决以下结构性问题：

1. 工作区 session 校验、in-flight 注册与 spool 选择并非同一原子动作，切换工作区时可能让旧请求继续运行或写入新工作区运行目录。
2. `with_workspace` 使用共享读锁包住可能写文件的闭包；并发 CRUD、导入和注册表读改写可能丢更新。
3. YAML/JSON 替换在 Windows 上存在“删除旧文件后再 rename”的空窗，多文件操作没有可恢复事务。
4. 响应 spool、元数据、索引和导入计划没有完整生命周期、硬资源预算或工作区归属。
5. `ActualRequest`、响应 header 和历史记录可携带 Authorization、Cookie、API key、解析后 query、请求 body 与本机路径。
6. 请求设置、变量解析、automation、代码生成与 importer 没有共享一条有效请求执行链，出现配置静默失效和同一请求多种行为。
7. 大 JSON 索引的缓存预算在构建完成后才生效；异常输入可在预算前耗尽内存。

### 1.3 不在本方案首轮范围内

以下事项不是本计划的首要交付，避免以“大重写”掩盖可靠性问题：

- 不把用户工作区 YAML 改为 SQLite 真相来源；SQLite 仍只适合可重建索引和运行态。
- 不重做 Vue 视图、交互设计或产品功能范围。
- 不在同一个阶段实现 OAuth2、WebSocket、gRPC 等新增协议能力。
- 不把“支持 Cookie / 重试 / 脚本”理解为先接线再补规范；不完整实现会制造更高风险。
- 不通过大量 `#[allow(clippy::...)]` 让现有门禁绿灯；接口形状问题应通过 DTO 和服务边界解决。

---

## 2. 目标状态与不可变约束

### 2.1 六条工程不变量

| 编号 | 不变量 | 可观察结果 |
| --- | --- | --- |
| I1 | 一个运行态对象只属于一个 `workspaceId + sessionId` | 工作区 A 的请求、响应、历史、索引、标签与导入计划不会被 B 读取、复用或写入 |
| I2 | session 失效后不再创建新副作用 | 晚到的自动保存、请求注册、历史写入和事件全部返回稳定错误或被静默丢弃，不会落入新 session |
| I3 | 同一工作区的持久化变更串行提交 | 两个命令同时创建集合、调试用例或环境时，结果要么都存在，要么有明确失败；不会发生最后写覆盖前写 |
| I4 | 一次持久化操作要么保持旧可读状态，要么留下可恢复的新状态 | 不允许因 rename、权限、磁盘满或进程中断造成 manifest 丢失、可见半导入或悬挂引用 |
| I5 | 大对象和秘密不跨越错误边界 | 响应 body 不走普通 IPC；默认 IPC、日志与历史不包含明文凭据、Cookie、Secret、完整 body 或本机绝对路径 |
| I6 | 执行前必须形成唯一的有效请求 | 手工发送、automation、代码生成、预览和导入后的请求共享变量、URL、继承、鉴权、body 与设置语义 |

### 2.2 目标运行态模型

`AppState` 不再分别维护全局 `spool`、`meta`、`semantic_meta`、`in_flight` 和可替换仓储。它只负责原子切换当前 `WorkspaceRuntime`：

```text
AppState
  └── current_runtime: Option<Arc<WorkspaceRuntime>>

WorkspaceRuntime
  ├── workspace_id + session_id + generation
  ├── repository: WorkspaceRepository
  ├── mutation_gate: 单工作区提交串行器
  ├── spool: Spool
  ├── responses: ResponseRegistry
  ├── in_flight: 请求取消与生命周期表
  ├── ui_state/history runtime 根目录
  └── scoped plan cache: ImportPlan / 临时分析结果
```

关键规则：

- 全局锁只用于读取、替换或使 `WorkspaceRuntime` 失效，**不得**包住磁盘扫描、导入、网络、索引构建或长时间文件复制。
- `begin_request(access, request)` 必须在一个临界区内完成 session 校验、response id 分配、取消句柄注册和 `Spool` 绑定，并返回 `RequestLease`。
- `RequestLease` 包含 runtime generation；完成、失败、进度和取消事件都必须验证 generation 仍有效。已关闭的 session 不得向新 session 写元数据或发送可消费的终结事件。
- 响应读取使用与 runtime 绑定的短期 capability，而非仅使用全局 UUID。`apogee://` URL 不能暴露真实文件路径。
- 所有写命令走 `runtime.mutate(...)`；读命令走短生命周期 `runtime.read(...)`。两者均先验证 `workspaceId + sessionId`。

### 2.3 目标持久化协议

所有用户工作区和 AppData 文件写入复用一套持久化基础设施：

```text
准备 Prepare
  └── 在同目录或同卷 staging 写入临时文件/目录，写完后校验可读性
提交 Commit
  └── 以 manifest 或状态文件作为可见性提交点；不删除最后一份有效目标
清理 Cleanup
  └── 成功后删除 tombstone / staging；失败时保留可恢复标记
恢复 Recover
  └── 启动或打开工作区时识别未完成事务，安全恢复、回滚或隔离
```

基础约束：

- 临时文件必须与目标在同一目录/卷；写入后显式 flush，替换前关闭句柄。
- Windows 使用不含“先删旧文件”空窗的 replace 原语；Unix 使用同目录 rename。平台对断电耐久性的差异必须写入实现注释和测试结论，不能把“进程级原子替换”笼统称为“断电绝不丢失”。
- 创建采用隐藏 staging，验证后以 manifest 最后提交；删除先移入 tombstone，提交 manifest 后再清理。
- 需要跨卷搬运的数据（例如 legacy history 到 AppData runtime）使用“复制到目标临时文件 → 校验 → 发布 → 删除源”的可重试流程，不能依赖 `rename`。
- 工作区同时被多个进程打开时必须有明确策略：首版采用单工作区单写者锁，无法获得锁时返回 `WORKSPACE_LOCKED`；未来多窗口协作前再设计跨进程事务协议。

### 2.4 目标请求执行模型

持久化模型与执行模型必须分离：

```text
RequestSpec（YAML / UI 中的模板）
  + 祖先容器设置、环境、应用默认设置
  ↓
EffectiveRequest（无 Inherit、无未解析变量、无静默 no-op）
  ↓ validate
RequestPlan（URL、header、body、超时、cookie、重试、代理已明确）
  ↓
RequestExecutor（HTTP / automation / CLI 共用）
  ↓
InternalResponseRecord ──> PublicResponseSummary（脱敏 IPC DTO）
```

`RequestSpec` 保持 Git 友好、可序列化和向后兼容；`EffectiveRequest` 只存在于执行期，禁止含 `Inherit`、残留 `{{variable}}` 或未支持的设置。任何入口都只能基于 `EffectiveRequest`：

- 手工发送、预览和调试用例；
- collection automation / runner；
- cURL、Postman、YApi、OpenAPI 等 importer；
- 代码片段生成与“等价 cURL”导出。

---

## 3. 实施顺序总览

| 优先级 | 阶段 | 主要产物 | 解决的核心风险 | 前置 |
| --- | --- | --- | --- | --- |
| P0 | 阶段 0：门禁与测试地基 | DTO 收敛、故障注入/并发测试支架、ADR | 修复当前 Clippy 门禁，避免改造无回归依据 | 无 |
| P0 | 阶段 1：Session 与 Runtime 原子绑定 | `WorkspaceRuntime`、`RequestLease`、响应 capability | 跨工作区串写、旧请求逃逸、旧响应可读 | 阶段 0 |
| P0 | 阶段 2：写串行化与可恢复持久化 | mutation gate、atomic replace、journal/staging/recovery | lost update、删除窗口、半导入、跨卷迁移失败 | 阶段 1 |
| P0 | 阶段 3：响应、资源与秘密边界 | `ResponseRegistry`、配额、脱敏 DTO、协议修复 | OOM、磁盘增长、凭据泄露、mmap 发布竞态 | 阶段 1 |
| P1 | 阶段 4：统一有效请求与导入管线 | `EffectiveRequest`、`ImportPlan`、支持矩阵 | 设置 no-op、变量语义分叉、导入语义丢失 | 阶段 2、3 |
| P2 | 阶段 5：模块边界、格式演进与长期质量 | service 拆分、错误契约、兼容层、基准 | 可读性、复用性、长期演进成本 | 阶段 1～4 |

阶段之间以验收门禁为界，不以“文件已拆分”作为完成标准。P0 未完成前，不应继续扩展大导入、自动化或多工作区能力。

---

## 4. 阶段 0：门禁与测试地基

### 4.1 交付内容

1. 将 `history_record`、`history_record_error`、`record_failed_history` 的长参数收敛为命令 DTO / 领域输入对象，例如 `HistoryWriteInput`、`FailedHistoryWriteInput`。
2. 在不改变用户格式的前提下，为以下测试建立共用支架：
   - 临时工作区和 runtime 目录；
   - 可控 mock HTTP server；
   - 可在指定文件操作后失败的 `FileOps`/failpoint 抽象；
   - 可暂停请求流程的 barrier，用于制造 close/open 与写入竞态；
   - 目录快照断言：manifest、集合目录、环境、history 和 spool 的可见状态。
3. 建立架构决策记录（ADR），至少锁定：
   - release panic 策略：`panic = "abort"` 与进程内 `catch_unwind` 不能同时作为可靠性承诺；
   - workspace 多进程写者策略；
   - 默认 runtime/spool 配额与历史保留策略；
   - 历史中敏感字段的默认保存策略。
4. CI 同时验证项目声明的 MSRV 和当前稳定工具链；当前本机 Clippy 发现的问题必须先修复，不能依赖旧工具链未报告。

### 4.2 Panic 策略决策门

现有 release 使用 `panic = "abort"`，但后台请求和 cURL parser 试图用 `catch_unwind` 转化为正常错误。两者语义冲突。

| 选项 | 适用条件 | 实施要求 |
| --- | --- | --- |
| 保持 `abort`（默认建议，待量化） | 包体预算仍要求该配置，且所有不可信/高风险代码不在主进程恢复 | 删除“panic 后仍会发终结事件”的承诺；普通错误全部走 `Result`；第三方不可信解析改为隔离 worker 或在入口前严格验证 |
| 改为 `unwind` | 实测包体/启动增量可接受，且需要主进程内任务级恢复 | 保留 `catch_unwind` 仅作最后防线；确保 Drop 清理、事件终结与锁中毒策略可测试 |

完成阶段 0 前必须选定其一并记录结果；不能继续保留“发布版可捕获 panic”的错误注释。

### 4.3 验收标准

- `cargo fmt --all -- --check`、MSRV 与 stable 的 `cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo test --workspace --locked` 全部通过。
- 新增测试可稳定在 CI 中制造文件失败和并发交错，不依赖 sleep。
- 不新增 `allow(clippy::too_many_arguments)`；命令层输入已经用 DTO 收敛。

---

## 5. 阶段 1：Session、请求与响应归属

### 5.1 改造范围

重点文件：

- `src-tauri/src/state.rs`
- `src-tauri/src/lib.rs` 的 request / response commands
- `src-tauri/src/protocol.rs`
- `crates/apogee-core/src/response.rs` 的接口边界

### 5.2 具体任务

1. 引入 `WorkspaceRuntime` 和不可伪造的 `WorkspaceAccess`/`RequestLease`。
   - `WorkspaceAccess` 是每个 workspace IPC 命令的输入，包含 `workspace_id`、`session_id`。
   - `RequestLease` 仅由 `begin_request` 生成，持有绑定 runtime 的 spool、取消句柄和 generation。
   - `finish_request` 只接受 lease，不能仅凭 response id 修改全局状态。

2. 将 `open_workspace` / `close_workspace` 改为运行态交换。
   - close 先使 runtime generation 失效并停止接受新 lease，再取消已有 in-flight；不等待网络任务持有全局锁。
   - open 创建完整新 runtime 后一次性发布；旧 runtime 只允许完成清理，不得进入新 runtime 的 registry。
   - 读取、索引、历史恢复和协议 handler 都从 capability 解析对应 runtime，而不是读取可被替换的全局 spool。

3. 改造响应读取 capability。
   - 前端收到的响应 URL 使用随机 capability，能力与 `workspaceId + sessionId + responseId + 生命周期` 绑定。
   - `get_response_meta`、`index_response`、`get_response_tree` 和协议读取都验证 capability 所属 runtime。
   - 关闭工作区后旧 capability 立即失效，返回稳定“响应已过期”错误，而不是误读新 spool 中同名文件。

4. 为 state 层加“所有权优先”的存储接口。
   - `meta: Vec<_>` 改为按 response id 查询的有界 registry；禁止同一 history response 重复 push。
   - `semantic_meta`、index、in-flight 和 plan cache 不再跨 runtime 共用。

### 5.3 必测场景

| 场景 | 断言 |
| --- | --- |
| A 工作区请求已完成 session 校验、尚未注册时关闭 A 并打开 B | 请求要么在注册前被拒绝，要么绑定 A runtime 后被取消；不得向 B 的 `responses/` 写文件 |
| A 的请求下载中切换到 B | A 的进度/终结事件不会更新 B；旧 response capability 无法读取 B 数据 |
| A 的自动保存、历史写入在 close 后到达 | 返回稳定 `WORKSPACE_SESSION_EXPIRED` 或被 lease 拒绝；B 的 YAML 不变 |
| 同一 response 被并发索引、Range 读取和恢复 | 每个操作只访问该 response 所属 runtime；不会访问全局当前 spool |

### 5.4 完成标准

- `AppState` 不再有可在 session 间切换的全局 `Spool`、跨工作区 metadata 或无归属 in-flight 表。
- 所有 response IPC 和自定义协议路径都具备 runtime 归属校验。
- 上表竞态测试在至少 100 次循环中稳定通过。

---

## 6. 阶段 2：写串行化、事务与恢复

### 6.1 改造范围

重点文件：

- `src-tauri/src/workspace.rs`
- `src-tauri/src/workspace_state.rs`
- `src-tauri/src/app_settings.rs`
- `src-tauri/src/workspace_registry.rs`

### 6.2 统一写入组件

提取单一持久化组件，例如 `infra::fs::{AtomicFile, WorkspaceTransaction, FileOps}`：

| 组件 | 职责 | 禁止事项 |
| --- | --- | --- |
| `AtomicFile` | 同目录临时写、关闭、替换、失败清理 | 先删除旧目标再 rename |
| `WorkspaceTransaction` | staging、tombstone、manifest 提交、恢复标记 | 多文件操作直接散落 `fs::rename/remove_*` |
| `FileOps` | 将真实文件系统包在可替换接口后 | 业务测试依赖磁盘满/杀软等不可控环境 |
| `MutationGate` | 串行化同工作区读改写提交 | 用 `RwLock` 的读锁当作写互斥 |

`MutationGate` 只保护提交事务，不包住网络分析、JSON 解码、Postman 文件读取或 UI 进度通知。大型 import 的正确流程是：在锁外解析为 `ImportPlan`，在 staging 构建并验证；仅在发布和 manifest 提交的短窗口内获取 mutation gate。

### 6.3 逐项迁移顺序

1. 先替换 `workspace_state`、settings、registry 的原子替换实现，清除所有“目标存在则 remove_file”的 fallback。
2. 将环境创建、保存、删除迁移为 transaction；保留并泛化现有 environment tombstone 正例。
3. 将 collection 创建、重命名、删除迁移为 transaction。
   - collection 删除必须先 rename 至 tombstone；manifest 成功提交后才能物理删除。
   - collection 重命名的 marker 与 manifest 必须在同一事务中保持可恢复一致。
4. 将 history body/YAML 写入、trim 与历史恢复迁移到统一 spool/history 发布路径。
5. 将 Postman 导入改为与 YApi 导入同级的 staging apply；禁止逐请求调用普通 CRUD。
6. 改造 legacy history 跨卷迁移为幂等 copy-publish-delete，并增加 migration marker；只要 legacy 仍存在，后续打开都能继续恢复。

### 6.4 提交与恢复语义

| 操作 | 提交点 | 中断后允许状态 | 恢复动作 |
| --- | --- | --- | --- |
| 创建集合/环境 | manifest 写入完成 | 不可见 staging 或完整可见对象 | 删除无引用 staging；保留完整提交 |
| 删除集合/环境 | manifest 移除引用完成 | 原对象或 tombstone | 未提交则恢复原位置；已提交则清理 tombstone |
| 保存单文件 | 原子替换成功 | 旧文件或完整新文件 | 清理临时文件 |
| 导入 | manifest 发布完成 | staging、完整导入或可回滚 tombstone | 依据 journal 恢复/清理，不允许“半导入可见” |
| 历史恢复 | spool 正式文件 rename 完成 | 无文件或完整文件 | 删除 `.part`，按长度/hash 决定是否重试 |

所有 rollback 清理失败都必须记录为可操作错误和恢复标记，不能用 `let _ = ...` 静默吞掉。

### 6.5 验收标准

- 两个并发 `create_collection`、两个并发 `create_debug_case`、保存请求与创建调试用例并发时，不丢任意成功操作。
- 每个事务关键步骤注入失败后，重新打开工作区仍能取得一致 snapshot，且不存在 manifest 引用缺失路径。
- Windows 上模拟目标存在、临时文件占用和第二次替换失败时，旧 manifest/state 文件仍存在或恢复流程能恢复。
- 同一工作区第二进程/第二窗口无法获得写者锁时返回稳定错误，不进行隐式并发写。

---

## 7. 阶段 3：响应资源、协议与敏感信息边界

### 7.1 ResponseRegistry 与资源预算

将 response 生命周期集中在 `ResponseRegistry`，记录 response 的 runtime 归属、size、访问时间、引用状态、索引状态和 capability，而不是仅靠 spool 目录与 `Vec` 元数据推断。

建议首版默认预算如下；最终值需在性能 Spike 和真实工作流验证后固化为应用设置：

| 资源 | 建议默认 | 强制语义 |
| --- | --- | --- |
| 单 HTTP 响应写入上限 | 保留现有 2 GiB 硬上限 | 超过即明确标记截断，不继续占用磁盘 |
| 单工作区 runtime spool | 500 MiB | 写入前预检，写入中保留全局预算；空间不足返回可操作错误 |
| 单个 JSON 索引 | 64 MiB | 构建前估算并硬拒绝，不能因“最新索引”绕过 |
| JSON 索引总缓存 | 128 MiB | LRU 只是淘汰策略，不是唯一保护 |
| 树节点单页 | 默认 200，服务端硬上限 500 | 前端参数不能要求一次返回全部子节点 |
| 待确认导入计划 | 有数量、大小与 TTL 上限 | session close 时释放；超时要求重新分析 |

若用户确实要保存大于 runtime quota 的下载，后续应走“另存为用户选择路径”的下载能力，而非放宽无界 spool。

### 7.2 Spool 发布、索引与协议

1. `SpoolWriter` 以“是否已发布”而不是 `file.is_some()` 决定 Drop 清理；flush 或 rename 失败也必须删除 `.part` 或留下可识别恢复标记。
2. **已完成（长度验证与不可覆盖发布）。** `Spool::copy_from` 复制到唯一临时文件、flush 后以同目录 hard link 原子发布；已有目标只有与 history source 长度匹配时才可作为幂等恢复复用。若发布窗口遇到竞争，只有 `AlreadyExists` 且长度匹配才可复用，大小不匹配、非冲突 I/O 错误和无法在该卷创建 hard link 的情况都不得转换为成功。发布成功或复用后的临时删除失败也必须返回错误，`Spool::remove` 会回收该 response 残留的 `.part`。`restore_history_response_into` 始终委托该校验，不再因 `exists()` 裸短路。若需要识别同长度损坏，再在此基础上增加可选 hash/完整性元数据。
3. JSON 索引改为有硬预算的构建器：
   - 在预分配前检查节点数与估算字节；使用 fallible reserve；
   - 对单容器 descriptor、对象键池和构建期临时内存设置上限；
   - 同 response 索引使用 single-flight，避免重复构建；
   - 优先演进为单遍显式栈扫描，避免嵌套容器反复扫描子树。
4. `protocol::handle` 区分无 Range、合法 Range 和非法/不可满足 Range：非法 Range 返回 `416` 与正确 `Content-Range`；无 Range 不得伪装为完整 200 响应却只返回前 4 MiB。
5. 协议响应使用记录的 content type / charset；HTML、图片、二进制和文本不再强制标为 JSON。协议 I/O 改用有上限的 blocking pool 或 semaphore，禁止每个请求无界 `std::thread::spawn`。

### 7.3 内部记录与公开 DTO 分离

```text
ActualRequest（执行内存）
  ├── 可含最终 URL/header/body 信息
  ├── 不序列化的敏感来源元数据（path/query/header）
  └── 仅 Rust 运行态短期持有

PublicRequestPreview
  ├── 默认脱敏 request 行与 URL
  ├── 不含 body、auth 或代理 URL
  └── 用于 history 与安全的不可执行预览

ResponseMetaInternal
  └── 包含 body_path，仅 Rust 侧使用

PublicResponseSummary / PublicError
  └── 不包含 body_path、明文凭据、不受限 request body 或 error detail
```

**本轮已实现。** `PublicActualRequest` 在从内部快照投影时使用 `ActualRequest` 的非序列化敏感来源元数据；这补足了纯名称 heuristic 无法识别的中性 `ValueKind::Secret`、自定义 API key 及编码 path 值。history 兼容读取则在原始 `PublicRequestPreview` 的 `Secret`/`File` 行被掩码前，将该来源应用到旧 actual snapshot。公开 DTO 不保存这份来源元数据。

必须默认脱敏的值包括：

- `Authorization`、`Proxy-Authorization`、`Cookie`、`Set-Cookie`；
- API key、签名、token、密码及 `ValueKind::Secret`；
- 含敏感名称的 query 参数；
- 二进制/文件上传的绝对路径；
- 完整请求 body，除非该字段被显式标记为可安全展示。

本轮**不提供 reveal**，也不自动清理或重写旧 history。若未来需要本地排障，必须先设计一次性、短时、显式触发且可审计的 reveal / 受控诊断导出，并以工作区绑定 capability 和过期语义取代默认明文；普通事件、日志、自动 history 和导出不得因为“方便调试”默认保存秘密。

### 7.4 验收标准

- 关闭工作区、关闭标签或超出 quota 后，spool、index、metadata 和 capability 均按明确规则回收；不会只清 history YAML 而遗留 runtime body。
- 对超预算 JSON，索引命令返回结构化资源错误而不是 OOM、panic 或不受限分配。
- 人工构造带 Bearer、Cookie、API key、Secret、文件路径和大 body 的请求时，普通 `request:done`、`request:error`、history YAML、日志和 `get_response_meta` 均无明文。
- malformed/suffix/over界 Range、空 body、HTML、图片、非 UTF-8 文本均有协议集成测试。

---

## 8. 阶段 4：统一 EffectiveRequest 与导入管线

### 8.1 请求设置支持矩阵

当前 schema 已能保存的字段不能继续以“保存成功但执行不生效”的方式存在。每个字段在进入 `EffectiveRequest` 时必须属于以下之一：

| 状态 | 语义 | 例子 |
| --- | --- | --- |
| 已实现 | 有端到端执行和测试 | timeout、TLS、代理、重定向、HTTP version |
| 明确拒绝 | 发送前返回 `UNSUPPORTED_REQUEST_SETTING` 与迁移建议 | 尚未支持却被持久化的字段 |
| 降级且可见 | 只允许兼容读取，返回 warning 并在 UI 标明 | 旧格式中的废弃字段 |

第一轮必须处理：

- `encode_url`：使用 `url::Url` 和组件级操作定义 path/query/fragment 语义；禁止整字符串 `replace` 与在 fragment 后追加 query。
- URL 变量：明确区分环境变量、`{{pathParam}}` 和 `:pathParam`；所有未解析模板在发送前失败，automation 不得有另一套宽松规则。
- `retry_count` / `retry_backoff_ms`：只对可重放、允许重试的请求执行；必须尊重取消、body 可重放性、幂等性和最大总时长。
- `send_cookies`：在 RFC 兼容 Cookie store 具备来源域、host-only、path 边界、secure、expiry/delete 和公共后缀策略前，不接入现有简化 `CookieJar`。

### 8.2 automation、代码生成与预览

- `automation::run_collection` 改为复用异步 `RequestExecutor`/`EffectiveRequest`，不再独立做 URL/Header 替换或把完整响应 body 强制收进 `String`。
- 受限脚本 DSL 必须按源码顺序处理；不支持的 JavaScript 不能静默返回成功。未来 QuickJS 接入前保持明确的语言/能力错误。
- 代码生成只从已验证的有效请求或脱敏 preview 生成；每种目标语言采用独立字符串/参数编码器，禁止将 method 或 URL 原样拼入 shell。
- `ActualRequest` 的公开预览遵循阶段 3 的脱敏策略，不再成为“为了生成代码而泄露凭据”的旁路。

### 8.3 ImportPlan 统一落盘

所有 importer 采用同一结构：

```text
输入文件/网络数据（有大小、深度、并发限制）
  ↓ Parser
provider-specific source model
  ↓ Converter
ImportPlan { tree, environments, warnings, unsupported_items }
  ↓ Preview / validate
WorkspaceTransaction::apply_import(plan)
  ↓
ImportReport
```

具体要求：

- Postman 对对象 URL 必须避免 raw query 与结构化 query 重复；按 `body.mode`、auth 和 script 正确转换，无法转换时写 warning。
- cURL 保留真实重定向语义，正确处理多段 data；不能因 `follow_redirects: None` 回落为应用默认 true。
- YApi 处理 `multipart/form-data` 时移除来源 boundary/header，让 HTTP 层生成新的 multipart boundary；网络读取增加大小上限。
- Postman、YApi 与未来 OpenAPI 都复用 staging transaction；禁止大型导入逐节点调用普通 CRUD。
- importer 的递归层级、输入体积、请求数量和并发均必须受限；来源内容视为不可信数据，不得携带对执行环境的指令。

### 8.4 验收标准

- 同一请求由手工发送、automation 和生成的等价 cURL 得到相同 URL、header、body 与 auth 语义（脱敏显示除外）。
- 未支持设置不会静默 no-op；每一个持久化设置都有端到端测试或明确拒绝测试。
- Postman/cURL/YApi 的代表语料覆盖 query、path、raw JSON、urlencoded、multipart、auth、重定向与 warning；导入失败不产生可见半成品。
- importer 解析与 apply 的性能测试证明大集合不再出现随节点数平方增长的全树扫描。

---

## 9. 阶段 5：模块边界、格式演进与长期质量

### 9.1 建议目录边界

不要求一次物理迁移到设计文档中的最终目录，但新代码应按以下职责进入独立模块，旧模块逐步迁出：

```text
src-tauri/src/
  commands/          # 仅 DTO 校验、调用 service、事件适配
  runtime/           # WorkspaceRuntime、session、response registry、capability
  workspace/         # repository、transaction、history、migration
  infra/             # fs replace、spool、protocol、platform adapter

crates/apogee-core/src/
  execution/         # EffectiveRequest、URL/body/auth/settings、HTTP executor
  import/            # ImportPlan 与 provider converter
  automation/        # runner、assertion、extractor，复用 execution
  model/             # 持久化与公开 DTO 的稳定边界
```

拆分原则：按“可独立测试的职责”拆，不按文件行数机械拆。先让依赖方向单向，再移动文件；避免在同一个 PR 同时改变行为、文件路径和序列化格式。

### 9.2 错误契约

所有用户可见的 Rust 错误收敛为可序列化 `CommandError/AppError`：

```rust
struct AppError {
    code: Option<String>,
    kind: ErrorKind,
    message: String,
    detail: Option<String>,
    hints: Vec<Hint>,
}
```

要求：

- `WORKSPACE_SESSION_EXPIRED`、`WORKSPACE_LOCKED`、`CACHE_QUOTA_EXCEEDED`、`UNSUPPORTED_REQUEST_SETTING`、`RESPONSE_CAPABILITY_EXPIRED` 等使用稳定 code，而非依赖 `to_string()`。
- `Internal` 如有必要，必须成为正式、前后端均声明的 `ErrorKind`，且仍具备 code/hint；不能临时拼 JSON 字符串。
- 构造用户可见错误时必须有至少一个 hint，或显式声明无可操作项的原因。
- IPC detail 不包含本地路径、完整敏感 URL 或凭据；完整诊断仅进入受保护的本地日志。

### 9.3 文件格式演进

- 用户工作区 YAML 维持 `apogee: 1`，只做可选字段新增和兼容读取；语义改变必须引入显式版本/迁移。
- 对 `Body`、`AuthConfig`、节点等 tagged enum 预留 unknown/unsupported 表示，避免未来字段让整个工作区无法打开。
- runtime/history 格式可独立版本化；它们是可重建或可隔离数据，但迁移失败不能阻塞用户读取工作区定义。
- 启动恢复在业务 UI 前执行并产出可诊断报告；损坏的运行态隔离备份，不覆盖用户工作区真相文件。

### 9.4 长期测试与性能门禁

| 类别 | 必须新增的覆盖 |
| --- | --- |
| 并发 | session close/open、并发 mutation、import 与保存并发、旧事件晚到 |
| 故障恢复 | 每个 transaction 提交点失败、跨卷迁移中断、spool restore 中断、临时文件清理 |
| HTTP | redirect、timeout、cancel、chunked、slow body、retry、Cookie、binary/form-data |
| 导入 | 真实语料回归 + source model 到 `EffectiveRequest` 的等价测试 |
| JSON | 深度、节点数、键池、预算拒绝、fuzz/property、单 response single-flight |
| 安全 | IPC/history/log 的 secret redaction 快照测试、capability 过期与跨 session 拒绝 |
| 性能 | 大响应、索引构建、导入大树、response LRU、内存/磁盘配额回收 |

现有 `cargo fmt`、Clippy、单元测试和 Windows E2E 继续保留；完成阶段 3、4 后，把故障恢复和资源预算测试纳入 CI 的目标化 job，而不是仅在本地手工运行。

---

## 10. 迁移、发布与回滚策略

### 10.1 数据迁移原则

1. 用户 YAML 是最高优先级资产，迁移不得静默删除或改变其业务语义。
2. runtime/history 是次级资产；发生错误时优先保护工作区，再隔离/重建运行态。
3. 每项迁移必须幂等：中断后重试不会重复复制、重复注册或遗漏未搬运文件。
4. 同一 release 不同时引入格式语义变化、文件系统事务重写和大功能新增；必要时使用 feature flag 或隐藏开关分批验证。

### 10.2 建议迁移路径

| 数据 | 迁移策略 | 回滚策略 |
| --- | --- | --- |
| 工作区 YAML/manifest | 保持 v1 reader，新增 writer 前先实现 read-after-write 校验 | 保留上一个完整文件或 tombstone，不接受半文件 |
| history 与响应体 | 通过 versioned runtime adapter 读取；新写入走事务化路径 | runtime 可隔离并重新恢复，不影响请求定义 |
| legacy history 跨卷迁移 | copy-publish-delete + marker，逐文件校验与重试 | 源文件在目标发布前不删除 |
| 敏感历史字段 | 先在读取/IPC 层脱敏；再提供用户确认的清理/重写 | 不默认删除旧记录，允许用户备份后处理 |
| response capability | 新 session 仅生成新 capability，旧 URL 自然过期 | 不依赖 capability 的持久化，关闭即可回收 |

### 10.3 发布门禁

每个 P0 阶段合入前必须满足：

- 所有阶段验收场景均自动化；
- Windows 上真实文件替换、协议与 runtime 生命周期测试通过；
- 70 个既有测试不回归，且新增场景不能只覆盖 happy path；
- `cargo fmt`、MSRV/stable Clippy、workspace test、前端 typecheck/test/E2E 按受影响范围通过；
- 性能 Spike 不出现超出阈值的启动、内存、10 MB 响应首屏或响应 spool 回归；
- release notes 明确说明 runtime/history 迁移、secret 脱敏变化和可能的用户操作。

---

## 11. 风险、决策点与缓解

| 风险 | 触发条件 | 缓解与决策门 |
| --- | --- | --- |
| 事务化改造扩大文件系统复杂度 | 同时改全部 CRUD | 先抽公共 replace + failpoint，再按环境、集合、history、import 分批迁移 |
| mutation gate 降低大导入吞吐 | 整个 import 持锁 | 解析和 staging 在锁外，仅发布窗口持锁；用进度和取消保持体验 |
| session capability 改动前端 URL | 协议 URL 格式变化 | IPC facade 一次性适配，旧 capability 只在当前会话短期兼容，不写入持久化 |
| 脱敏降低排障信息 | 用户需要原始请求 | 显式 reveal/受控诊断导出替代默认泄露，并记录安全审计理由 |
| Cookie/retry 实现复杂 | 不完整实现先上线 | 先拒绝 no-op 配置；Cookie 采用经过审查的 store 或完整 RFC 行为测试后再接线 |
| `panic=abort` 与恢复诉求冲突 | 未作 ADR 就继续写 catch | 阶段 0 做包体/可靠性实验后固定策略；禁止混合语义 |
| 新资源上限影响大文件用户 | 默认 quota 过小 | 返回可操作错误和另存能力，不偷偷截断；设置层让用户可理解地调整上限 |
| 多进程打开同一目录 | 无锁或锁失效 | 首版明确单写者，加入 lock 生命周期与崩溃恢复测试；多窗口能力后置 |

---

## 12. 审查发现到实施阶段的映射

| 审查发现 | 实施阶段 |
| --- | --- |
| session 校验/请求注册/spool 绑定竞态 | 阶段 1 |
| 读锁下写、registry/workspace lost update | 阶段 1、2 |
| Windows 删除再 rename、多文件非事务 | 阶段 2 |
| history restore 直接写最终 spool、mmap 不变量 | 阶段 2、3 |
| spool/meta/plan 无界增长 | 阶段 3 |
| JSON index 预算后置与重复扫描 | 阶段 3 |
| IPC/history 明文携带凭据、body 与路径 | 阶段 3 |
| Range/MIME/无界协议线程 | 阶段 3 |
| RequestSettings no-op、URL 变量分叉 | 阶段 4 |
| Cookie、retry、automation 与主 HTTP 链路不一致 | 阶段 4 |
| Postman/cURL/YApi 导入语义与复杂度 | 阶段 4 |
| commands/workspace 巨型模块、String 错误 | 阶段 0、5 |
| panic abort 与 catch_unwind 矛盾 | 阶段 0 |
| 文件格式演进、测试覆盖、platform unsafe | 阶段 5；`mem.rs` 安全修复可独立提前 |

---

## 13. 最终验收定义

本方案完成不等于“代码拆成更多模块”，而是同时满足以下结果：

1. 切换 A → B 工作区时，A 的任何旧请求、响应、索引、历史和自动保存都不能读写 B。
2. 并发写入、导入中断、磁盘错误、Windows 替换失败和跨卷迁移失败后，用户工作区仍可打开，且可见数据一致或可自动恢复。
3. 响应与导入资源受硬预算约束；超限得到明确错误而不是 OOM、磁盘写满或无界缓存。
4. 默认 IPC、历史、日志和代码生成不会泄露凭据、Cookie、Secret、完整 body 或本机路径。
5. 所有已暴露的请求设置均被执行、显式拒绝或可见降级；不会再出现“配置保存了但什么也没发生”。
6. 手工发送、automation、导入与代码生成共享同一有效请求语义。
7. Rust Clippy、格式、编译、测试和新增并发/故障恢复/资源预算门禁在 MSRV 与 stable 工具链上持续通过。

达到这些标准后，再推进 Cookie、Postman 完整兼容、Runner、CLI、多窗口或更多协议，新增能力才不会建立在会串写、会丢数据或会泄露秘密的基础上。
