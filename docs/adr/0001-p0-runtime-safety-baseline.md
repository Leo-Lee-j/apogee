# ADR-0001：P0 运行态可靠性与数据安全基线

> 状态：已接受
>
> 日期：2026-08-21
>
> 范围：release panic、多进程工作区写入、运行时资源预算、历史数据保留与敏感信息边界。
>
> 关联：[Rust 工程可靠性优化实施方案](../05-Rust工程可靠性优化实施方案.md)

## 背景

Apogee 的工作区 YAML 是用户数据真相，运行时响应、索引与历史则属于可隔离、可重建或可清理的数据。当前实现已经具备单响应的传输上限、按数量裁剪历史、针对部分写入失败的回归测试、history/IPC/普通日志的默认脱敏公开 DTO，以及 P0 阶段 2 的最小单写者/原子写/环境恢复切片；但尚未形成统一的运行时资源预算或覆盖全部多文件操作的持久化事务协议。

同时，release profile 使用 `panic = "abort"` 以满足包体预算，但请求任务中仍保留 `catch_unwind`。abort 发生时进程会在任何 Rust 恢复路径前结束，因此不能把它视为 release 下发送终结 IPC 事件的可靠性机制。

本 ADR 为 P0 后续实现设定不可变约束；除明确标注为“已实现”的事项外，决策本身不表示现有代码已完整具备对应能力。

## 实施状态更新（2026-08-21）

本次已落地 response runtime 的 session 隔离、**P0 默认脱敏子切片**与 **P0 阶段 2 最小持久化切片**；它不代表 P0 完成，也不改变本 ADR 的完整预算与剩余事务迁移决策：

- 已实现 `ResponseAccess` / `ResponseScope`、`RequestLease` 和 session 私有 `Arc<ResponseRuntime>`。普通 response IPC、协议读取、索引、history restore、Synthetic 及前端请求事件均按 `workspaceId + sessionId` 校验；旧 runtime 仅在最后一个锁外任务结束后回收其私有 spool。
- 已实现 session 切换时清理 response metadata/index，并以 barrier 测试证明旧 request、history restore、index 与 Synthetic 不能向新 session 发布或写入其目录。此机制是隔离与延迟回收，不是 500 MiB runtime 配额或跨进程所有权协议。
- 已完成部分单文件发布改进：Windows 替换不再先删除旧目标；history body 恢复先写唯一 `.part`、flush 后以同目录 hard link 不可覆盖发布。`Spool::copy_from` 会把长度匹配的已有 target 作为幂等恢复复用；若在发布窗口发生竞争，只有明确的 `AlreadyExists` 且长度匹配才会复用 body。大小不匹配、其他发布/I/O 错误和不支持 hard link 的卷都会保持失败；成功发布或复用后的临时删除失败也返回错误，`Spool::remove` 会回收该 response 残留的 `.part`。`restore_history_response_into` 也不再使用 `exists()` 裸短路。它不等于多文件事务、断电恢复或跨进程单写者锁。
- **已实现阶段 2 最小持久化切片。** `WorkspaceWriteLock` 对 canonical root 的 `.apogee.lock` 持有 Windows `LockFileEx` / Unix `flock` advisory exclusive lock，session 与活动 mutation 都保留该引用，第二个 Apogee 进程/窗口会得到 `WORKSPACE_LOCKED`。`MutationGate` 串行化工作区写命令、history 与 import apply；`WorkspaceAccessGate` 让普通读取共享、提交和同根 reload recovery 独占，避免旧 manifest 与文件清理交错。共享 `atomic_write` 已用于 settings、workspace state、registry 和 YAML。环境 create/delete 已有 UUID journal、staging/tombstone 与恢复；create 为 `journal → staging → publish → manifest`，delete 以 manifest 为提交点再清理 target/tombstone。该范围不包含所有 collection/request/history/import 的完整事务迁移，也不保证任意断电时机的目录耐久性。
- 当前普通 response URL 依赖 workspace/session 归属校验，尚不是目标模型中的随机、短期、不可伪造 response capability；完整 `WorkspaceRuntime`、`ResponseRegistry` 和资源预算仍待实现。
- **已实现默认脱敏公开边界（已按安全复审补强）。** `PublicRequestPreview`、`PublicActualRequest`、`PublicResponseSummary`、`PublicError` 和 `PublicHint` 分离内部执行模型与 history/IPC 载荷。history YAML 的读写、失败 history、`request:done` / `request:error`、response metadata/index、Spike 响应与前端 history IPC 都只传递公开 DTO；完整 body、auth、代理 URL、文件路径、error detail 和自由文本 description 不会进入该边界。前端 preview 会基于中性 `Secret` query 名、大小写不同的编码 path 值和自定义 query/header API-key 名先行净化 URL 与 preview 行。
- **已实现来源感知的实际请求脱敏。** `ActualRequest` 在内存中携带不可 serde 的 path/query/header 敏感来源；HTTP snapshot 从 `ValueKind::Secret`/`File`、Bearer/Basic 和 API key 采集来源，公开投影同时遮蔽中性字段名（如 `id`、`q`、`X-My-Key`）及 URL 编码后的 path 值（包括大小写不同的百分号十六进制）。配置的自定义 API-key Header 即使被同名手写 Header 覆盖也保留为敏感来源；实际请求和 history preview 的同名行都会遮蔽。旧 history 的 legacy `auth` 只反序列化无值的 `type`/`key`/`location` 脱敏来源，因此自定义 API-key Header/Query 同样会遮蔽 legacy preview 行、失败 `actualRequest` 和成功 response 内嵌的 actual request。
- **兼容与范围。** 用户工作区请求 YAML 保持原始 source of truth；旧 history 可读但不自动重写或删除。普通日志仅记录安全摘要。history 以 tab-level `historyPreview` 打开，`useRequest().send()` 在执行入口拒绝它，因此 UI 以外的调用也不能重新发送历史预览。本切片不提供 reveal、受控诊断导出或 history 自动清理；这些能力必须以后续 capability、过期、审计和显式同意设计为前提。
- **剩余完整性边界：**当前 `Spool::copy_from` 校验的是来源和已发布 body 的长度，因此能拒绝截断或不同长度的错误文件，但不能区分同长度损坏；如需该保证，应通过后续 ADR/实现引入可选 hash 或完整性元数据，并明确其 I/O 成本。

## 决策

### 1. release 保持 `panic = "abort"`

**决策：** 继续采用 `panic = "abort"`。任何预期失败都必须通过 `Result`、取消或明确的领域错误返回；release 不承诺在 panic 后继续运行、清理运行态或发送 `request:error`。

- `catch_unwind` 仅可作为支持 unwind 的开发/测试配置下的诊断性兜底，不能成为用户可依赖的正常错误语义。
- 解析器、网络和文件系统边界必须首先消除可预期 panic；需要隔离不可信第三方代码时，后续通过独立 worker 或明确输入校验实现，而不是依赖进程内恢复。
- 只有重新量化包体、启动和错误恢复收益后，才可通过新的 ADR 改为 `panic = "unwind"`。

**已实现状态：** release profile 已使用 abort；现有注释会明确 `catch_unwind` 的限制。完整的 panic 审计属于后续 P0 交付。

### 2. 工作区采用单进程、单写者策略

**决策：** 一个工作区根目录同一时刻只允许一个 Apogee session 持有写权限。

- `workspace_open` 必须取得基于工作区根目录稳定路径（建议 `.apogee.lock`）的操作系统 advisory exclusive lock，并持有到 `workspace_close` 或进程退出。
- 无法取得锁时，命令返回稳定错误码 `WORKSPACE_LOCKED`；首版不提供隐式只读回退、多个写者合并或多窗口协作。
- 锁的所有权依赖操作系统进程生命周期释放；锁文件是否存在不能单独作为“正在占用”的依据。
- 未来的多窗口或跨进程协作必须重新设计事务、冲突和恢复协议，不得绕过此锁。

**已实现状态：** 已实现阶段 2 最小单写者切片：OS advisory lock 绑定 session 生命周期，不能获取锁时返回 `WORKSPACE_LOCKED`；同根 reload 复用现有 lock 时会先等待旧 mutation 完成，避免 recovery 观察活动 journal 的中间态。该实现仍是单写者协议，不保护绕过 Apogee 直接修改目录的外部工具；collection/request/history/import 与跨卷迁移的完整事务化仍待后续阶段 2 迁移。

### 3. 运行时数据使用硬预算

**决策：** 每个工作区 runtime 采用以下默认硬上限；上限由后续设置层以受限、可理解的方式配置，不能由前端请求参数绕过。

| 资源 | 默认上限 | 强制语义 |
| --- | ---: | --- |
| 单个 HTTP 响应传输保护上限 | 2 GiB | 保留现有绝对安全上限；不能无界写盘 |
| 工作区 runtime 的响应与历史 body 共享预算 | 500 MiB | 写入前预检、写入中持续记账；达到上限返回可操作错误 |
| 单个 JSON 索引 | 64 MiB | 在预分配/构建前拒绝超限输入 |
| JSON 索引总缓存 | 128 MiB | 按 workspace runtime 的 LRU 加硬限制，而不是构建后才淘汰 |
| 历史记录 | 最多 200 条 | 保留现有数量语义；body 同时计入 500 MiB runtime 预算 |
| 树节点单页 | 默认 200、最大 500 | IPC 参数不能要求一次返回无界节点 |

用户需要保留超出 runtime 预算的响应时，后续必须使用显式的“另存为”目标路径；不得静默截断、放宽全局上限或把大对象塞进普通 IPC。

**已实现状态：** 2 GiB 单响应上限、128 MiB 现有索引缓存和 200 条历史数量限制已存在；统一 runtime 记账、构建前索引预算和可操作的超限错误安排在 P0 阶段 3。

### 4. 历史、IPC 与日志默认脱敏

**决策：** 工作区 YAML 仍按产品语义保存用户定义的请求；但 history、普通 IPC 事件、错误 detail 和日志不得默认复制或暴露原始执行秘密。

- 新历史模型应以 `HistoryRequestView`/`PublicResponseSummary` 代替原始 `RequestSpec`、`ActualRequest`、`body_path` 和不受限 body。
- 默认脱敏 `Authorization`、`Proxy-Authorization`、`Cookie`、`Set-Cookie`、API key、签名、token、密码、`ValueKind::Secret`、敏感 query 参数、二进制上传的本机绝对路径和完整请求 body。
- 响应 body 可以作为受预算的本地 runtime 文件保留，但普通 history/IPC 不自动传递；查看原文需要用户显式触发且使用短期、工作区绑定 capability。
- 旧历史采用读兼容：先阻止旧明文再次进入默认 IPC，再提供用户确认的清理或重写工具；不静默删除用户数据。

**已实现状态：**P0 默认脱敏子切片已实现并完成安全复审后的补强。公开 DTO 覆盖 history YAML 读写、请求终结事件、response metadata/index、相关 Spike 响应、前端 history IPC 与普通请求终结日志；旧 history 读兼容时再净化，且不会自动改写用户历史。执行期 `ActualRequest` 的来源元数据补足了中性 Secret/API key、API-key Header 覆盖以及大小写不同的百分号编码路径值的遮蔽；legacy `auth` 仅保留无值的名称/位置来源，用于净化旧 preview、失败与成功 actual snapshot。前端 history preview 也会在执行入口被拒绝。仍未实现 reveal、受控诊断导出、用户确认的旧 history 清理/重写，以及阶段 3 的资源预算/协议改造；它们不得成为重新暴露默认明文的理由。

## 实施约束与顺序

1. 阶段 0 固化本 ADR，并在 CI 同时运行声明的 MSRV 与当前 stable 的 Rust 质量门禁。
2. 阶段 1 用 `WorkspaceRuntime`、`RequestLease` 和 response capability 绑定 session、spool、in-flight 和响应读取。
3. 阶段 2 已完成单写者锁、`MutationGate`、`WorkspaceAccessGate`、共享原子写与环境 create/delete 的 journal/recovery 最小切片；剩余 CRUD、history、import 和跨卷迁移仍须在锁之上实现可恢复事务，不能把锁当成多文件事务的替代品。
4. 阶段 3 完成共享 runtime 配额、`ResponseRegistry`、索引预算与协议资源边界；公开 DTO 默认脱敏子切片已提前落地，后续工作不得绕过该边界。

在阶段 2、3 的剩余迁移完成前，不扩展多工作区写入、大型导入、自动化或“可恢复 panic”承诺。

## 后果

- 对用户：达到预算或工作区被占用时会得到明确错误，而非继续写入、覆盖或静默降级；敏感调试信息需要显式 reveal 流程。
- 对开发：新增功能必须声明其运行时归属、资源成本、历史持久化内容和错误语义；不能通过 `#[allow(clippy::...)]`、全局缓存或字符串拼接错误绕过本 ADR。
- 对迁移：用户 YAML 不迁移为 SQLite；runtime/history 可版本化和隔离，但故障不得阻断用户读取工作区定义。

## 验收与复审

- CI 的 MSRV 和 stable 均执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings` 与 `cargo test --workspace --locked`。
- 每个后续 P0 阶段必须在对应功能实现和故障/并发测试后更新本 ADR 的“已实现状态”。
- 任何调整 panic 策略、工作区并发模型、默认预算或历史秘密处理方式的变更，都必须新增或修订 ADR，并附带迁移、回滚和用户可见行为说明。
