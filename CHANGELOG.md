# 变更记录

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 的结构，并以 [Semantic Versioning](https://semver.org/lang/zh-CN/) 管理发布版本。未发布的工作先记录在 `Unreleased`；只有实际发布的标签才应获得发布日期。

## [Unreleased]

### Added

- 增加可取消的异步认证准备 / HTTP 执行边界，桌面发送入口在 Rust 侧兜底准备持久化或继承 OAuth2；前端保留 Inherit 来源。草稿 / 用例显式认证的凭据更新仍使用专用敏感通道，不在普通发送中丢失旋转凭据。
- core 增加顺序异步 Runner 基础：逐项 prepare → execute、迭代变量隔离、失败停止、独立取消和有界结果流；尚未提供桌面 Runner 命令、跑批 UI 或 CLI。
- OAuth2 增加有效期保存及请求 / 集合 / 文件夹 Auth 的可选发送前刷新：默认关闭、提前 30 秒刷新、刷新失败不发送，继承请求更新真实来源并保持 Inherit；手动与自动操作统一在 Rust 侧按来源串行，凭据先保存，环境 / 配置 / 会话变更拒绝旧结果。自动化 / CLI 暂未接入，不在 401 后重放请求。
- OAuth2 增加授权码 + PKCE S256：系统浏览器授权、固定端口本机回调、公共客户端、取消和会话隔离；Postman 配置导入导出及旧 YAML 兼容。

- 请求、集合和文件夹支持 OAuth 1.0（HMAC-SHA1 / HMAC-SHA256）、OAuth 2.0 已有 Access Token 和 JWT（HS256 / HS384 / HS512），共用 Auth 编辑器并支持变量替换。

- OAuth2 支持 Client Credentials 获取与手动 Refresh Token，支持 Basic / Body 客户端凭据、环境变量、集合/文件夹网络设置继承、凭据轮换和取消。

- 请求参数支持 Query String/完整 URL 查询部分与表格批量粘贴、无损批量编辑及格式错误提示。

- 以 MIT 许可证公开 Apogee 源码。
- 贡献、安全、社区行为与发布流程文档。
- 面向 Windows 的 CI、依赖更新与发布自动化（发布前仍需由仓库管理员配置 GitHub 安全与签名设置）。

### Changed

- Rust 最低版本从 1.85 提升至 1.89，CI 与发布工具链同步锁定 1.89.0；该版本覆盖现有锁文件及新 YAML 编解码依赖的要求。
- YAML 读写统一使用 core 层的 `serde-saphyr 2.1.1` 安全编解码边界；保留 Format v1 模型语义、历史只读兼容和原子发布，输出布局可能变化。单文档输入最多 64 MiB、嵌套深度最多 32，过深或超限文件会明确拒绝。

### Fixed

- 后端认证准备失败不进入业务请求历史，不构造实际请求快照；显式持久化认证校验完整来源基线，旧调用或无来源调用不能冒充可回写请求。
- OAuth2 轮换凭据使用来源 Auth 比较更新；普通请求和容器保存增加 Auth 基线校验，旧编辑页不能覆盖新凭据。短有效期 Token 保留轮换后的 Refresh Token 并停止业务发送；配置页提供重新加载。

- URL userinfo 不再绕过 Auth 面板或与手写 Authorization Header 重复发送。
- Postman v2.0 / v2.1 导入保留 Basic、Bearer、API Key 和新增鉴权配置，展开上级继承、保留 urlencoded Body，并避免 URL 对象中的 Query 被重复添加；不支持的认证明确警告。

- 新参数行原子提交、form-data 草稿类型与文件选择隔离，避免字段拆成多行或晚到结果写入其他请求。
- Path 参数只扫描 URL 路径并按完整名称替换；Query 参数在 URL fragment 前追加，避免漏发。

- 集合删除和重命名增加可恢复事务：清单发布失败时保留原集合，中断后恢复目录清理和名称一致性，并拒绝不安全的集合事务路径。

### Security

- PKCE 授权码与 verifier 不进入 IPC、配置或历史；回调限制来源、方法、路径、state、重复参数、大小、数量和等待时间，失败不回显凭据或服务端描述。
- 移除停止维护的 `serde_yaml_ng` / `unsafe-libyaml` 依赖，恢复完整漏洞、许可证、来源与依赖禁用门禁，不新增安全公告忽略规则。限制 YAML 节点、事件与别名扩展，关闭隐式 include/properties 等扩展；解析和序列化错误仅保留安全阶段与数值位置，不携带源片段或凭据。
- 升级 Vue 至 3.5.42、source-map-js 至 1.2.2、rustls 至 0.23.46、rustls-webpki 至 0.103.16、Tokio 至 1.53.3，修复对应依赖安全公告。
- Token 请求仅访问 HTTPS（本机调试允许 HTTP），禁止自动重定向，响应限制 64 KiB；错误脱敏，不进入普通响应快照、历史或 spool。取消覆盖环境保存、登记前竞态和网络读取，工作区/环境/请求切换后丢弃旧结果。

- 新鉴权的签名错误、响应快照、历史和文档均脱敏；OAuth1 不转发重定向旧签名，API Key 拒绝空名称及 Cookie 分隔符注入。签名复用现有 ring 后端。

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