# 第三轮问题修复与复评：变量作用域与密钥边界

> 历史评估记录：保留当时的基线、发现和验证结果。当前版本说明见[文档索引](../README.md)与[更新记录](../../CHANGELOG.md)。

评估日期：2026-11-20。代码基线：`d985d01`（包含前两轮已推送修复）。本文先记录修复结果与回归，再保留修复前审查证据。实现和测试使用合成密钥，未接触真实凭据。

## 修复后状态与验证

修复前确认了密钥明文存储/展示、多作用域缺失，以及 host 预览优先级不一致。修复后，标记为密钥的环境和全局变量通过系统凭据存储；集合、文件夹、环境和运行时作用域进入统一解析；host 冲突会被拒绝，预览按相同优先级构造。

| 发现 | 修复结果 | 回归证据 |
| --- | --- | --- |
| P1 密钥持久化、IPC 与快照脱敏 | YAML 只存稳定变量 ID 与密钥引用；快照返回空值和已设置状态；发送时在 Rust 内部取值；解析保留敏感来源并清理 URL、Header、路径与查询公开快照。Postman Environment 的 secret 标记会进入同一安全保存流程；集合/文件夹 secret 被跳过并给出不含值的警告。 | 合成值验证环境/全局保存、复制、删除、Postman 导入；检查环境 YAML 与公开投影；HTTP 快照测试覆盖自定义 Header、Query、Path。Rust 库测试使用进程内凭据存储；桌面 E2E 使用真实 Windows 提供器和合成值。 |
| P2 多作用域 | 优先级为 Runtime > Environment > 最近文件夹 > Collection > Global。禁用项不会遮蔽外层，同一作用域重复名拒绝。普通发送与测试专用自动化适配器共享解析器；无活动环境时补全仍显示全局/容器变量。Postman 集合和文件夹普通变量导入后也会随集合导出。 | Core 作用域测试、工作区文件夹合并测试、测试适配器本地 HTTP 测试，以及前端无环境/覆盖/host 预览测试；自动化仍没有生产桌面 Runner UI。 |
| P2 host 预览 | host 为保留名，环境、全局、容器和运行时层均拒绝普通 host 声明；补全使用环境 Host 并统一去除末尾斜线。 | Rust 解析测试、前端预览测试与 UI 保存校验。 |

验证结果：Core 库 185 项、桌面端库 116 项、前端 22 个测试文件共 178 项通过；前端类型检查及生产构建通过；严格 Clippy、格式检查、依赖安全与许可检查通过。首屏静态 JS/CSS gzip 为 103,054 字节（100.64 KiB），低于 150 KiB 门禁。Windows 桌面 E2E 通过，使用隔离工作区和合成凭据验证真实凭据读写、复制、普通请求的全局/容器/环境变量覆盖及 YAML/IPC/UI/历史脱敏，结束删除测试凭据。Linux/macOS 凭据提供器尚未实测。旧版环境 YAML 没有密钥标记，无法安全推断哪些既有普通变量应自动迁移，用户需将敏感项标记为密钥并保存。

密钥替换生成新的凭据引用，文件发布成功后再清理旧引用；创建/保存发布失败会清理新凭据并保留旧值。复制环境同时复制禁用密钥，发送则跳过禁用密钥读取。清空或尚未设置的密钥被请求引用时返回变量错误。Header/参数名称与值、通用 Header 名单及 URL 均按解析来源脱敏。敏感文件路径解析后保留 multipart 的 File 类型，避免将上传静默改为文本。全局保存合并在途任务，发送前等待保存期间产生的新编辑，旧会话响应不会覆盖新工作区。

解析边界：每个作用域最多 256 条、原始名称和值合计 65,536 字节，运行时注入同样校验；最多 16 层嵌套、10,000 次变量展开、单个变量结果最多 1 MiB、一次请求累计展开计算最多 8 MiB。大段无变量请求体不受变量展开预算影响。前端预览也限制展开次数和结果长度，覆盖空值指数引用与大结果引用；UTF-16 预览长度仅用于本地保护，发送时以 Rust 字节限制为准。

本机验证使用 Rust stable 1.99.1；CI 要求的 MSRV 1.89.0 未在本机复测。未运行完整性能基准、安装包或跨平台发行验收。

以下各节是修复前的发现与证据，用于说明问题来源；不表示当前实现仍保留这些问题。

| 顺序 | 级别 | 发现 | 影响 |
| --- | --- | --- | --- |
| 1 | P1 | 环境密钥默认明文持久化、传入前端并显示；Postman 的密钥类型未迁移 | 工作区副本或 Git 提交可能带出凭据；实际请求快照的脱敏规则不能可靠识别环境密钥 |
| 2 | P2 | 只支持单一活动环境作用域 | 全局、集合、文件夹和运行时覆盖及变量来源链无法表达；Postman Collection 变量只能收到迁移警告 |
| 3 | P2 | 保留变量 `host` 与环境 Host 的预览优先级不一致 | 建议值与发送实际采用的 Host 可能不同，候选项也可能重复 |

## 1. P1：环境密钥没有安全存储和来源标记

`EnvironmentVariable` 当前只有 `key`、`value`、`enabled`，没有 `secret` 或来源字段（`crates/apogee-core/src/vars.rs:26-35`）。环境数据文件按工作区普通 YAML 读取、保存为 `environments/<uuid>.yaml`（`crates/apogee-core/src/workspace.rs:27-30`、`src-tauri/src/workspace.rs:748-750,1375-1387`）。每次加载工作区的 `WorkspaceSnapshot` 会包含完整环境对象，随后由打开工作区命令返回前端（`crates/apogee-core/src/workspace.rs:94-100`、`src-tauri/src/lib.rs` 的 open-workspace 路径）。

变量编辑器使用普通文本输入，绑定并显示完整 `variable.value`；请求编辑器的变量候选会解析并展示变量值（`src/components/layout/EnvironmentControls.vue:274-280`、`src/components/request/VariableInput.vue:35-45`、`src/components/request/JsonBodyEditor.vue:53-65`）。工作区根目录 `.gitignore` 仅忽略 `*.local.yaml` / `*.local.json`，实际生成的 `<uuid>.yaml` 环境文件不匹配这些规则（`.gitignore:29-34`）。产品设计文档已要求密钥加密、UI 打码、不入 Git，以及使用系统钥匙串；当前依赖和实现中没有 keyring 接入（`docs/01-产品设计文档.md:291-293,529-533`、`docs/03-技术架构与数据模型.md:20-27`）。

Postman Environment 导入只读取变量名、值和 enabled 状态，忽略其他属性且没有对密钥类型给出警告（`crates/apogee-core/src/postman.rs:75-99`）。因此即便源文件明确标记敏感值，当前导入模型仍只保存普通字符串。

还有一条与日志/历史安全相关的代码路径：Rust 将环境变量解析成普通字符串；变量模型没有敏感来源，解析结果也没有携带密钥 provenance（`crates/apogee-core/src/vars.rs:48-67,75-122`）。之后实际请求快照根据请求行自身的 `ValueKind`、Header/参数名称和少数值前缀判断脱敏（`crates/apogee-core/src/http.rs:1450-1500,1570-1610`）。若密钥变量填入名称不显敏感的自定义 Header，且值不匹配已知前缀，来源信息无法进入 `PublicActualRequest` 的敏感 Header 名单。这是代码链路确认的条件性泄露风险；本轮没有用真实值或桌面网络发送实测。

验收应覆盖：敏感变量经 OS credential store 持久化，工作区 YAML 只存稳定引用；公开 IPC 默认返回脱敏值与敏感标记；前端显式揭示才短暂显示；Postman 导入遇到 secret 类型必须安全写入或阻止并说明，不得默默明文保存；变量解析结果携带敏感来源，确保自定义 Header、Query、Path 及实际请求/历史投影均打码。测试使用合成凭据并检查 YAML、IPC DTO、日志、历史和导出中均无原值。

## 2. P2：全局、集合与运行时作用域尚未交付

解析器只接收一个 `Option<&Environment>`，从该环境及其 Host 建立单个 HashMap；请求发送上下文只携带 `environment_id`，没有运行时变量集合。`ContainerDocument` / `WorkspaceContainer` 也没有变量字段（`crates/apogee-core/src/vars.rs:48-52,75-122`、`crates/apogee-core/src/request.rs:79-105`、`crates/apogee-core/src/workspace.rs:45-72`）。因此现有行为是“活动环境变量 + Host”，不包含 Global、Collection、Folder 或 Runtime 覆盖，也不返回变量解析来源链。

设计文档规定优先级为 Runtime > Environment > Collection > Global > 动态内置（`docs/03-技术架构与数据模型.md:551-565`），产品需求将作用域列为 v0.2 Must（`docs/01-产品设计文档.md:286-295`）。Postman 集合导入对 Collection/Folder 的 `variable` 只产生“未迁移”警告（`crates/apogee-core/src/postman.rs:291-310` 的 `migration_gaps`）。这保留了当前请求本身，但迁移后的请求需要手动把变量搬进环境，且无法保留不同集合的同名变量隔离。

验收：先固定大小写、最近作用域覆盖、禁用/空值语义、Path 与环境 URL 兼容规则；再由 Rust 构建一个有界的解析上下文，统一用于普通发送和自动化，返回脱敏的来源 trace；导入时映射 Collection/Folder/Global/Environment 并报告无法映射项。需测试同名覆盖、循环引用、缺失值、并发环境切换、请求取消和跨工作区隔离。

## 3. P2：内置 `host` 的补全预览与实际解析可能不一致

Rust 构造环境变量映射时先插入普通变量，随后以 Environment.host 覆盖 `host`（`crates/apogee-core/src/vars.rs:100-120`）。前端的变量候选则先把 Host 放入数组，再追加普通变量，预览映射按顺序以最后一个同名键为准（`src/components/request/VariableInput.vue:40-44`、`src/features/variablePreview.ts:13-18`；JSON 编辑器也采用相同顺序）。环境配置只验证普通变量彼此重名，没有把保留名 `host` 纳入冲突检查（`src/components/layout/EnvironmentControls.vue:19-25,47-54`）。

若环境 Host 为 `https://staging.example.test`，同时添加普通变量 `host=https://other.example.test`，补全候选可能把两项都预览成普通变量的值，而发送解析中的 `host` 使用环境 Host。建议将 `host` 明确设为只读内置名并拒绝同名，或令前端和 Rust 使用相同优先级，并对 UI 补全做回归。此项是源代码路径推导，尚未做桌面交互复现。

## 修复前评估的验证范围（历史记录）

- `cargo test -p apogee-core --locked vars::tests --quiet`：7 项通过。
- `cargo test -p apogee-core --locked imports_postman_environment_current_values --quiet`：1 项通过；现有测试只验证普通值、current 值、enabled 和缺失 key，没有覆盖 secret 类型。
- `pnpm exec vitest run src/features/variablePreview.test.ts`：2 项通过；当前测试没有覆盖 `host` 重名。
- 检查了工作区快照、YAML 持久化、变量编辑/补全、Rust 发送准备与敏感快照、Postman 导入和现有设计约束。

修复前评估阶段未修改产品代码，也没有运行桌面 E2E 复现敏感快照路径。随后实施的修复和回归见本文开头；全部使用合成数据，没有使用真实凭据。
