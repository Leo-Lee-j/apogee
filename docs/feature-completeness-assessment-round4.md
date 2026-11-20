# 第四轮复评：认证凭据与变量作用域

评估日期：2026-11-20。基线：已推送的 `eac40c2`。本轮在第三轮修复后继续审查认证路径；即时修复一个旧环境兼容性回归，其余发现记录如下。使用合成数据和本地测试，没有调用外部认证服务或读取真实凭据。

## 已修复：旧环境变量 ID 导致 OAuth2 误判配置变化

旧 YAML 只有 key/value/enabled，没有变量 ID。新增模型在读取时为缺少的 ID 生成 UUID；两次读取同一旧文件，编辑身份不同，但执行值相同。OAuth2 原先把整个 Environment 序列化进 issuance context，并在 Token 返回后直接比较环境对象，可能产生 `OAUTH2_SOURCE_STALE`、丢弃有效轮换结果。

修复：issuance context 排除变量行 ID、hasValue 和 clearSecret 等编辑元数据；提交前重新计算语义上下文，继续检查真实环境值、配置、认证来源和网络设置变化。没有关闭并发保护，也不需要用户先保存一次旧环境。

回归：核心测试两次读取同一旧结构，确认行 ID 不同而上下文相同，修改变量值后上下文不同；桌面宿主测试使用本地 HTTP Token 服务，在旧环境文件缺少变量 ID 时成功提交轮换。原有环境变更、配置变更、取消与会话关闭保护仍通过。

## 剩余发现

| 顺序 | 级别 | 发现 | 影响与边界 |
| --- | --- | --- | --- |
| 1 | P1 | 直接填写的 Auth 凭据及获取到的 OAuth2 Token 仍进入工作区 YAML | 环境/全局密钥已安全存储，但不能据此认为所有认证凭据均已脱离可分享目录；复制或提交请求/容器 YAML 仍可能带出 Token、密码或客户端密钥 |
| 2 | P2 | OAuth2 获取/刷新 Token 仍只解析活动环境 | 普通请求能使用 Global/Collection/Folder/Runtime，Token URL、Client ID/Secret、Scope 与 Refresh Token 的解析路径无法接收这些作用域；上下文指纹和提交校验也未包含它们 |
| 3 | P3 | 只有全局变量未保存时，环境配置仍可能显示“已保存” | 保存流程与退出流程会刷新全局变量，未确认数据丢失；问题是状态提示遗漏，容易误导用户 |

### 1. Auth 凭据持久化边界

证据：`crates/apogee-core/src/request.rs` 的 AuthConfig 对 Basic/Bearer/OAuth1/OAuth2/JWT/API Key 的凭据字段直接派生 Serialize。`src-tauri/src/workspace.rs` 的 save_request（约 1947 行）、save_container（约 1184 行）和 compare_and_set_auth（约 1388 行）把这些对象写回请求或容器 YAML，没有对应的私有凭据存储模型。`crates/apogee-core/src/oauth2.rs::apply_auth_response` 把返回的 Access Token 和 Refresh Token 放入 AuthConfig，managed 操作提交时再调用 compare_and_set_auth。

合成复现：使用当前编译的 apogee-core，在临时目录把 Bearer 与带 Refresh Token 的 OAuth2 RequestSpec 序列化并写入 YAML。结果：两类 YAML 均保留合成凭据；PublicRequestPreview 不包含该凭据。该结果确认持久化边界问题，同时说明公开历史预览的脱敏没有因此失效。临时文件已自动清理。没有额外执行桌面 Auth 保存/导出操作；其落盘链路由以上源码确认。

建议验收：为请求/容器 Auth 引入与环境分离的持久化模型，实际凭据存系统凭据存储，工作区文件只存引用；OAuth2 获取/轮换使用同一机制。覆盖原值迁移、复制、删除、失败回滚、继承、工作区隔离和导出安全策略。不要把公共 DTO 脱敏直接用于覆盖可执行认证配置。

### 2. OAuth2 的作用域不完整

证据：`crates/apogee-core/src/oauth2.rs::resolve`（约 198 行）逐字段调用 vars::resolve_text(text, environment)，没有 VariableScopes 参数。`src-tauri/src/oauth2/managed.rs::snapshot_in`（约 66 行）只读取所选环境；issuance_context 也只接环境和设置。普通 HTTP 执行则使用 resolve_request_with_scopes，拥有全局、集合、文件夹和运行时值。

定向检查：合成 global 变量 client 可以解析普通请求的 X-Client Header；OAuth2 实际调用的 environment-only 文本解析器在没有活动环境时拒绝同一引用。没有向 Token 端点发请求；这是解析器验证与调用链审查，不能表述为已完成桌面 OAuth2 端到端复现。

建议验收：普通发送、手动获取、自动刷新和 PKCE 交换共用冻结的变量上下文；凭据变更指纹与提交前校验覆盖所有有效作用域，并保留来源继承和运行时覆盖规则。覆盖只配置 Global 的客户端凭据、文件夹覆盖、刷新期间作用域变更、取消和旧会话响应。

### 3. 全局变量的保存状态提示

`src/components/layout/EnvironmentControls.vue` 的按钮脏标记及弹窗底部状态（约 186、399–400 行）只看 workspace.activeEnvironmentDirty，没有合并 globalVariablesDirty。仅编辑全局变量时，底部仍可能显示“已保存”。`src/stores/app.ts::prepareExit` 与 workspace.saveDirtyEnvironments 已包含全局保存，故本项仅按显示问题记录，不推断为退出丢失。

建议：合并两种脏状态显示，并覆盖无活动环境、仅全局修改、保存中再次修改与保存失败。

## 验证范围与下一步

- Rust 工作区测试：303 项通过（core 186、desktop 117），严格 Clippy 与格式检查通过。
- 前端：22 个文件、178 项通过；类型检查及生产构建通过。此次 OAuth2 兼容修复没有修改前端。
- 首屏静态 JS/CSS gzip：103,054 字节（100.64 KiB），通过 150 KiB 门禁。
- Windows 真实桌面 E2E：工作区、编辑、Body、Postman、Cookie、多作用域、系统凭据及公开快照/历史脱敏回归通过；新增旧环境 Token 场景由宿主本地 HTTP 测试覆盖。
- 新增 keyring 依赖已通过 cargo-deny 与 npm 生产依赖安全/许可检查；本轮追加修复没有新增依赖。
- 未运行 macOS/Linux 凭据提供器、Rust 1.89.0 MSRV、完整性能基准或安装包验收。

下一轮优先完成 Auth 凭据持久化边界，再统一 OAuth2 的变量上下文；全局保存状态提示可作为局部修正。当前报告不把核心库与测试专用 Runner 适配器视为已交付桌面自动化。
