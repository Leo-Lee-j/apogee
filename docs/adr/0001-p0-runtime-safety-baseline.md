# ADR-0001：P0 运行态可靠性与数据安全基线

> 状态：已接受
>
> 日期：2026-08-21
>
> 范围：release panic、多进程工作区写入、运行时资源预算、历史数据保留与敏感信息边界。
>
> 关联：[Rust 工程可靠性优化实施方案](../05-Rust工程可靠性优化实施方案.md)

## 背景

Apogee 的工作区 YAML 是用户数据真相，运行时响应、索引与历史则属于可隔离、可重建或可清理的数据。当前实现已经具备单响应的传输上限、按数量裁剪历史，以及针对部分写入失败的回归测试；但尚未形成统一的运行时资源预算、历史脱敏策略或跨进程写入协议。

同时，release profile 使用 `panic = "abort"` 以满足包体预算，但请求任务中仍保留 `catch_unwind`。abort 发生时进程会在任何 Rust 恢复路径前结束，因此不能把它视为 release 下发送终结 IPC 事件的可靠性机制。

本 ADR 为 P0 后续实现设定不可变约束；除明确标注为“已实现”的事项外，决策本身不表示现有代码已完整具备对应能力。

## 实施状态更新（2026-08-21）

本次落地的是 response runtime 的 session 隔离子切片，**不改变**本 ADR 的单写者、预算和默认脱敏决策，也不代表 P0 完成：

- 已实现 `ResponseAccess` / `ResponseScope`、`RequestLease` 和 session 私有 `Arc<ResponseRuntime>`。普通 response IPC、协议读取、索引、history restore、Synthetic 及前端请求事件均按 `workspaceId + sessionId` 校验；旧 runtime 仅在最后一个锁外任务结束后回收其私有 spool。
- 已实现 session 切换时清理 response metadata/index，并以 barrier 测试证明旧 request、history restore、index 与 Synthetic 不能向新 session 发布或写入其目录。此机制是隔离与延迟回收，不是 500 MiB runtime 配额或跨进程所有权协议。
- 已完成部分单文件发布改进：Windows 替换不再先删除旧目标；history body 恢复先写唯一 `.part`、flush 后发布。它不等于多文件事务、断电恢复或跨进程单写者锁。
- 当前普通 response URL 依赖 workspace/session 归属校验，尚不是目标模型中的随机、短期、不可伪造 response capability；完整 `WorkspaceRuntime`、`ResponseRegistry`、`MutationGate` 和资源预算仍待实现。
- **安全边界仍未达标：**失败 history 目前会持久化原始 `RequestSpec`、`ActualRequest` 和 error detail，可能包含凭据、Cookie、token、body 或本机路径。必须先引入默认脱敏公开 DTO 与回归测试，才能宣称满足本 ADR 的历史/IPC/日志策略。
- **下一个正确性修复：**`Spool::copy_from` 仍需仅把明确的“目标已存在”错误视为并发发布成功，并验证已有 body；不能把任意 rename 失败吞为成功。

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

**已实现状态：** 尚未实现，安排在 P0 阶段 2 的 `MutationGate` 与持久化事务改造中。

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

**已实现状态：** 尚未完整实现，安排在 P0 阶段 3。当前历史仍可能持久化原始请求和执行快照，不能被误认为满足该策略。

## 实施约束与顺序

1. 阶段 0 固化本 ADR，并在 CI 同时运行声明的 MSRV 与当前 stable 的 Rust 质量门禁。
2. 阶段 1 用 `WorkspaceRuntime`、`RequestLease` 和 response capability 绑定 session、spool、in-flight 和响应读取。
3. 阶段 2 在单写者锁之上实现 `MutationGate`、staging、tombstone、journal 与恢复；不得把锁当成多文件事务的替代品。
4. 阶段 3 实现共享 runtime 配额、`ResponseRegistry`、索引预算和公开 DTO 脱敏。

在阶段 2、3 完成前，不扩展多工作区写入、大型导入、自动化或“可恢复 panic”承诺。

## 后果

- 对用户：达到预算或工作区被占用时会得到明确错误，而非继续写入、覆盖或静默降级；敏感调试信息需要显式 reveal 流程。
- 对开发：新增功能必须声明其运行时归属、资源成本、历史持久化内容和错误语义；不能通过 `#[allow(clippy::...)]`、全局缓存或字符串拼接错误绕过本 ADR。
- 对迁移：用户 YAML 不迁移为 SQLite；runtime/history 可版本化和隔离，但故障不得阻断用户读取工作区定义。

## 验收与复审

- CI 的 MSRV 和 stable 均执行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings` 与 `cargo test --workspace --locked`。
- 每个后续 P0 阶段必须在对应功能实现和故障/并发测试后更新本 ADR 的“已实现状态”。
- 任何调整 panic 策略、工作区并发模型、默认预算或历史秘密处理方式的变更，都必须新增或修订 ADR，并附带迁移、回滚和用户可见行为说明。
