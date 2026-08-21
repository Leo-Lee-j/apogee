//! 应用状态：响应体 spool + JSON SpanIndex 缓存 + 请求中注册表。

use crate::workspace::WorkspaceRepository;
use anyhow::{bail, Context, Result};
use apogee_core::model::{ResponseMeta, Timing};
#[cfg(test)]
use apogee_core::public::PublicRequestPreview;
use apogee_core::public::PublicResponseSummary;
use apogee_core::response::{
    scalar_preview, synth_json_payload, ResponseStore, ResponseSummary, SpanIndex, Spool, TreeNode,
    PREVIEW_READ_BYTES,
};
use memmap2::MmapOptions;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tokio::sync::oneshot;
use uuid::Uuid;

pub struct WorkspaceSession {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub repository: WorkspaceRepository,
}

/// 单个 response runtime 的引用计数所有者。
///
/// 所有可能在 workspace 锁外读写 response body 的路径都必须持有 `Arc<Self>`。
/// 活跃 runtime 被替换后，最后一个旧 lease 释放时才删除其私有 spool 目录，因此
/// 旧请求可以安全收尾，却永远不会触碰新 session 的文件。
pub(crate) struct ResponseRuntime {
    spool: Spool,
    spool_root: PathBuf,
}

impl ResponseRuntime {
    fn new(spool_root: PathBuf) -> Result<Arc<Self>> {
        let spool = Spool::new(&spool_root)?;
        Ok(Arc::new(Self { spool, spool_root }))
    }

    pub(crate) fn spool(&self) -> &Spool {
        &self.spool
    }
}

impl Drop for ResponseRuntime {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.spool_root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = %self.spool_root.display(),
                    ?error,
                    "回收已关闭 session 的 response spool 失败"
                );
            }
        }
    }
}

/// 正在飞行的请求。
///
/// 取消用 `oneshot::Sender` 而不是 `CancellationToken`：
/// `apogee_core::http::execute` 的签名就是 `oneshot::Receiver<()>`，
/// 直接用 sender 当注册表条目，取消时 `send(())` 一发即达，不需要桥接层。
struct InFlight {
    cancel: oneshot::Sender<()>,
    #[allow(dead_code)]
    tab_id: Option<String>,
}

/// 只能由 [`AppState`] 在验证 workspace session 后创建的响应访问凭据。
///
/// 字段保持私有，命令层只能把它原样交回 `AppState`；这样后台请求完成时不会仅凭
/// response id 向当前（可能已替换为另一个工作区的）全局响应状态写入数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseAccess {
    workspace_id: Uuid,
    session_id: Uuid,
}

impl ResponseAccess {
    /// 事件载荷需要携带此身份，让前端丢弃已切换工作区后才送达的消息。
    pub fn workspace_id(self) -> Uuid {
        self.workspace_id
    }

    pub fn session_id(self) -> Uuid {
        self.session_id
    }
}

/// 响应记录的可见范围。正常响应必须匹配创建它的 workspace session；仅 Spike 的
/// 合成响应可以在没有工作区 session 时访问。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseScope {
    Workspace(ResponseAccess),
    Synthetic,
}

/// Synthetic 生成开始时捕获的 runtime 身份。
///
/// 它既区分活跃 workspace session，也持有 runtime lease。这样启动页生成期间发生
/// open → close 的 ABA 切换时，旧生成任务也不能把结果注册到新的启动页 spool，且
/// 旧 spool 会等到实际写入结束后才被回收。
#[derive(Clone)]
struct SyntheticRuntime {
    runtime: Arc<ResponseRuntime>,
    workspace_access: Option<ResponseAccess>,
}

/// 只能由 [`AppState::begin_request`] 创建的 session 绑定请求能力。
///
/// 创建时已在 session 读锁内固定 response spool、响应访问凭据并注册取消 receiver；
/// 网络任务一旦取走它，就不会在工作区切换后意外改用新 session 的 spool 或 metadata。
pub struct RequestLease {
    runtime: Arc<ResponseRuntime>,
    cancel: oneshot::Receiver<()>,
    response_access: ResponseAccess,
}

impl RequestLease {
    /// 交给 HTTP 执行器的资源只能消费一次，避免重复使用同一个取消 receiver。
    /// runtime lease 会持续到后台任务完全结束，之后才能回收已关闭 session 的 spool。
    pub(crate) fn into_execution_parts(
        self,
    ) -> (Arc<ResponseRuntime>, oneshot::Receiver<()>, ResponseAccess) {
        (self.runtime, self.cancel, self.response_access)
    }
}

/// 响应摘要索引：响应 ID、传输体积、耗时和 Content-Type。体不进入此缓存。
type ResponseMetaEntry = (Uuid, u64, Timing, Option<String>);

pub struct AppState {
    /// 当前活跃 response runtime。替换时只释放 AppState 的一份 Arc；仍在收尾的
    /// 请求、history、Synthetic 或索引任务继续持有旧 runtime，直至安全回收。
    runtime: RwLock<Arc<ResponseRuntime>>,
    pub store: ResponseStore,
    /// 只存元信息（体积、耗时），**不存 body**。
    pub(crate) meta: RwLock<Vec<ResponseMetaEntry>>,
    semantic_meta: RwLock<HashMap<Uuid, ResponseSummary>>,
    /// 每条响应的 session 归属。查询、索引和协议读取都必须匹配此记录。
    response_scopes: RwLock<HashMap<Uuid, ResponseScope>>,
    /// 当前窗口绑定的工作区会话。启动入口阶段为 None。
    pub workspace: RwLock<Option<WorkspaceSession>>,
    /// 尚未确认落盘的 YApi 导入计划。计划不包含项目 Token。
    pub yapi_plans: RwLock<HashMap<Uuid, apogee_core::yapi::YapiImportPlan>>,
    /// 请求中注册表。key 是 ExecHandle.id（= 响应 id）。
    in_flight: RwLock<HashMap<Uuid, InFlight>>,
}

impl AppState {
    fn new_startup_runtime() -> Result<Arc<ResponseRuntime>> {
        let root = std::env::temp_dir()
            .join("apogee-spool")
            .join(Uuid::new_v4().to_string());
        ResponseRuntime::new(root)
    }

    pub fn new() -> Result<Self> {
        let runtime = Self::new_startup_runtime()?;
        Ok(Self {
            runtime: RwLock::new(runtime),
            store: ResponseStore::new(),
            meta: RwLock::new(Vec::new()),
            semantic_meta: RwLock::new(HashMap::new()),
            response_scopes: RwLock::new(HashMap::new()),
            workspace: RwLock::new(None),
            yapi_plans: RwLock::new(HashMap::new()),
            in_flight: RwLock::new(HashMap::new()),
        })
    }

    fn clear_response_state(&self) {
        self.response_scopes
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.meta
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.semantic_meta
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.store.clear();
    }

    pub fn open_workspace(
        &self,
        repository: WorkspaceRepository,
        runtime_root: impl AsRef<std::path::Path>,
    ) -> Result<(Uuid, Uuid)> {
        let workspace_id = repository.id()?;
        let session_id = Uuid::new_v4();
        // response body 只属于本次 session；同一 workspace 重开也绝不复用旧 body。
        // 在取得全局写锁前创建唯一目录，避免目录 I/O 延长工作区切换临界区。
        let runtime = ResponseRuntime::new(
            runtime_root
                .as_ref()
                .join("responses")
                .join(session_id.to_string()),
        )?;
        let mut guard = self
            .workspace
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // 正常流程前端会先 close 再 open；这里仍残留会话，说明是前端重载或中途
        // 失败遗留的孤儿会话（前端已回到启动页且丢失 sessionId，无法再 close）。
        // 直接替换而不是报 WORKSPACE_ALREADY_OPEN，否则用户会卡死在启动页，
        // 无论点哪个工作区都进不去。
        let replaced = guard.is_some();
        // 仍持有 workspace 写锁时清除旧响应域，避免新 session 在旧 metadata/index
        // 尚未清理的窗口内复用旧 response id。
        self.clear_response_state();
        let old_runtime = {
            let mut runtime_guard = self
                .runtime
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::replace(&mut *runtime_guard, runtime)
        };
        *guard = Some(WorkspaceSession {
            id: session_id,
            workspace_id,
            repository,
        });
        drop(guard);
        if replaced {
            // 旧会话的请求全部作废，与 close_workspace 语义一致。
            self.cancel_all_inflight();
        }
        // 不在 workspace 锁内释放旧 Arc；没有旧任务时其 Drop 会立即回收目录，有
        // 任务时则留给最后一个 runtime lease 在安全收尾后回收。
        drop(old_runtime);
        Ok((workspace_id, session_id))
    }

    pub fn with_workspace<T>(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        operation: impl FnOnce(&WorkspaceRepository) -> Result<T>,
    ) -> Result<T> {
        let guard = self
            .workspace
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let session = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("当前没有打开的工作区"))?;
        if session.workspace_id != workspace_id || session.id != session_id {
            bail!("WORKSPACE_SESSION_EXPIRED");
        }
        operation(&session.repository)
    }

    /// 在同一 session 临界区内将工作区仓储与其 response runtime 提供给短事务。
    ///
    /// 锁顺序与 `open_workspace`/`begin_request` 一致：先读取 workspace，再读取 runtime。
    /// 长路径可以在闭包中 clone `Arc<ResponseRuntime>`，释放锁后继续持有 runtime lease；
    /// 这保证旧 session 的目录不会在锁外 I/O 期间被提前回收。
    fn with_workspace_runtime<T>(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        operation: impl FnOnce(&WorkspaceRepository, &Arc<ResponseRuntime>) -> Result<T>,
    ) -> Result<T> {
        let workspace_guard = self
            .workspace
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let session = workspace_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("当前没有打开的工作区"))?;
        if session.workspace_id != workspace_id || session.id != session_id {
            bail!("WORKSPACE_SESSION_EXPIRED");
        }
        let runtime_guard = self
            .runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        operation(&session.repository, &runtime_guard)
    }

    /// 在同一 session 临界区内执行既依赖工作区仓储、又依赖指定响应 body 的短操作。
    ///
    /// 回调获得 owning runtime lease 而非裸 `Spool`；若未来确实需要锁外工作，只能
    /// clone 该 `Arc` 以延迟目录回收，不能脱离 runtime 保存 spool 路径。
    pub(crate) fn with_workspace_and_response<T>(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        response_id: Uuid,
        operation: impl FnOnce(&WorkspaceRepository, &Arc<ResponseRuntime>) -> Result<T>,
    ) -> Result<T> {
        let access = Self::response_access(workspace_id, session_id);
        self.with_workspace_runtime(workspace_id, session_id, |repository, runtime| {
            let scope = self
                .response_scopes
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&response_id)
                .copied();
            match scope {
                Some(ResponseScope::Workspace(owner)) if owner == access => {
                    operation(repository, runtime)
                }
                Some(_) => bail!("RESPONSE_SESSION_EXPIRED"),
                None => bail!("RESPONSE_NOT_FOUND"),
            }
        })
    }

    pub fn close_workspace(&self, workspace_id: Uuid, session_id: Uuid) -> Result<()> {
        // 提前创建启动页 runtime，使目录 I/O 不进入 workspace 写锁临界区；若随后发现
        // session 已失效，局部 Arc 会立即删除这个尚未发布的临时目录。
        let startup_runtime = Self::new_startup_runtime()?;
        let mut guard = self
            .workspace
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let session = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("当前没有打开的工作区"))?;
        if session.workspace_id != workspace_id || session.id != session_id {
            bail!("WORKSPACE_SESSION_EXPIRED");
        }
        // 在释放 workspace 写锁前清理所有响应域，防止并发 open 发布新 session 后被
        // 旧 close 清空新 session 的 metadata/index。
        self.clear_response_state();
        let old_runtime = {
            let mut runtime_guard = self
                .runtime
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::replace(&mut *runtime_guard, startup_runtime)
        };
        *guard = None;
        drop(guard);
        self.cancel_all_inflight();
        // cancellation 已发出、全局锁已释放后才交出旧 runtime 的 AppState 所有权。
        // 仍在运行的任务持有各自的 Arc，因此不会与目录回收发生写入竞争。
        drop(old_runtime);
        Ok(())
    }

    /// Invalidate every request when its workspace session closes. The frontend
    /// may already have disposed the request store by the time cancellation
    /// events arrive, so cancellation is best-effort and does not wait here.
    fn cancel_all_inflight(&self) {
        let mut guard = self
            .in_flight
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (_, inflight) in guard.drain() {
            let _ = inflight.cancel.send(());
        }
    }

    /// 在同一 workspace 读锁临界区内校验 session、绑定 response spool 并注册取消 receiver。
    ///
    /// `open_workspace` 需要该锁的写权限后才能替换 spool，`close_workspace` 也必须先
    /// 取得写锁再取消 in-flight；因此 lease 返回后要么仍属于已验证的 session，要么
    /// 在随后 close 时收到取消信号，而不会绑定到新工作区的 spool。
    pub fn begin_request(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        request_id: Uuid,
        tab_id: Option<String>,
    ) -> Result<RequestLease> {
        let guard = self
            .workspace
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let session = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("当前没有打开的工作区"))?;
        if session.workspace_id != workspace_id || session.id != session_id {
            bail!("WORKSPACE_SESSION_EXPIRED");
        }

        let response_access = ResponseAccess {
            workspace_id,
            session_id,
        };
        let runtime = self
            .runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let cancel = self.register_inflight(request_id, tab_id);
        let lease = RequestLease {
            runtime,
            cancel,
            response_access,
        };
        drop(guard);
        Ok(lease)
    }

    /// 注册一个正在执行的请求，返回 receiver 给 execute() 当取消信号。
    fn register_inflight(&self, id: Uuid, tab_id: Option<String>) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        let mut guard = self
            .in_flight
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(id, InFlight { cancel: tx, tab_id });
        rx
    }

    /// 取消一个请求。`true` = 找到了并发了取消信号。
    /// `oneshot::send` 消费 sender，取消后该条目自动失效。
    pub fn cancel_inflight(&self, id: Uuid) -> bool {
        let mut guard = self
            .in_flight
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(inflight) = guard.remove(&id) {
            let _ = inflight.cancel.send(());
            true
        } else {
            false
        }
    }

    fn response_access(workspace_id: Uuid, session_id: Uuid) -> ResponseAccess {
        ResponseAccess {
            workspace_id,
            session_id,
        }
    }

    /// 短暂捕获当前 Synthetic runtime。生成 payload 与写盘都必须在它返回后进行，
    /// 以便 close/open 可以立即取得 workspace 写锁；返回值中的 Arc 会让旧 spool 持续
    /// 存活到锁外写入完成。
    fn capture_synthetic_runtime(&self) -> SyntheticRuntime {
        let workspace_guard = self
            .workspace
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let workspace_access = workspace_guard.as_ref().map(|session| ResponseAccess {
            workspace_id: session.workspace_id,
            session_id: session.id,
        });
        let runtime = self
            .runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        SyntheticRuntime {
            runtime,
            workspace_access,
        }
    }

    fn synthetic_runtime_expired(runtime: &SyntheticRuntime) -> anyhow::Error {
        if runtime.workspace_access.is_some() {
            anyhow::anyhow!("WORKSPACE_SESSION_EXPIRED")
        } else {
            anyhow::anyhow!("SYNTHETIC_RUNTIME_EXPIRED")
        }
    }

    /// 在锁外写完 Synthetic body 后，验证其 runtime 仍是捕获时的那一个，再发布
    /// Synthetic scope 和 metadata。Arc 身份避免启动页的 ABA 切换。
    fn with_current_synthetic_runtime<T>(
        &self,
        captured: &SyntheticRuntime,
        operation: impl FnOnce(&Spool) -> Result<T>,
    ) -> Result<T> {
        let workspace_guard = self
            .workspace
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let workspace_access = workspace_guard.as_ref().map(|session| ResponseAccess {
            workspace_id: session.workspace_id,
            session_id: session.id,
        });
        if workspace_access != captured.workspace_access {
            return Err(Self::synthetic_runtime_expired(captured));
        }
        let runtime_guard = self
            .runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !Arc::ptr_eq(&runtime_guard, &captured.runtime) {
            return Err(Self::synthetic_runtime_expired(captured));
        }
        operation(runtime_guard.spool())
    }

    /// 在 session 仍然有效时执行一个短终结操作，例如向前端投递请求终态事件。
    ///
    /// 该闭包在 workspace 读锁内执行，所以 `close_workspace` 不能在验证和事件入队
    /// 之间插入。事件本身仍携带 session，前端会在异步送达时再作一次身份过滤。
    pub fn with_active_response_access<T>(
        &self,
        access: ResponseAccess,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        self.with_workspace(access.workspace_id, access.session_id, |_| operation())
    }

    /// 在当前 workspace session 与指定响应归属一致时，向短生命周期读操作提供 runtime。
    ///
    /// 需要在锁外读取、解析或写入 body 的调用方必须在闭包中 clone `Arc`，以延迟旧
    /// session spool 的回收；普通 Range/tree 读取仍只使用下面的 `with_response`。
    fn with_response_runtime<T>(
        &self,
        access: ResponseAccess,
        response_id: Uuid,
        operation: impl FnOnce(&Arc<ResponseRuntime>) -> Result<T>,
    ) -> Result<T> {
        let workspace_guard = self
            .workspace
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let session = workspace_guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("当前没有打开的工作区"))?;
        if session.workspace_id != access.workspace_id || session.id != access.session_id {
            bail!("WORKSPACE_SESSION_EXPIRED");
        }
        let runtime_guard = self
            .runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scope = self
            .response_scopes
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&response_id)
            .copied();
        match scope {
            Some(ResponseScope::Workspace(owner)) if owner == access => operation(&runtime_guard),
            Some(_) => bail!("RESPONSE_SESSION_EXPIRED"),
            None => bail!("RESPONSE_NOT_FOUND"),
        }
    }

    /// 在当前 workspace session 与指定响应归属一致时，向短生命周期读操作提供 spool。
    fn with_response<T>(
        &self,
        access: ResponseAccess,
        response_id: Uuid,
        operation: impl FnOnce(&Spool) -> Result<T>,
    ) -> Result<T> {
        self.with_response_runtime(access, response_id, |runtime| operation(runtime.spool()))
    }

    /// 为明确的 Synthetic response 提供 runtime lease。它不依赖 workspace session，
    /// 但 runtime 身份仍必须保持一致，避免锁外索引或生成跨越切换后访问新 spool。
    fn with_synthetic_runtime<T>(
        &self,
        response_id: Uuid,
        operation: impl FnOnce(&Arc<ResponseRuntime>) -> Result<T>,
    ) -> Result<T> {
        let _workspace_guard = self
            .workspace
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let runtime_guard = self
            .runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scope = self
            .response_scopes
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&response_id)
            .copied();
        match scope {
            Some(ResponseScope::Synthetic) => operation(&runtime_guard),
            Some(_) => bail!("RESPONSE_SESSION_EXPIRED"),
            None => bail!("RESPONSE_NOT_FOUND"),
        }
    }

    /// Spike 的合成响应不依赖 workspace session，但也不能成为普通响应的无鉴权旁路。
    /// 只有明确标记为 Synthetic 的响应才能走此路径。
    fn with_synthetic_response<T>(
        &self,
        response_id: Uuid,
        operation: impl FnOnce(&Spool) -> Result<T>,
    ) -> Result<T> {
        self.with_synthetic_runtime(response_id, |runtime| operation(runtime.spool()))
    }

    /// 仅接受由 [`RequestLease`] 提供的 access 写入 HTTP 响应元数据。
    ///
    /// 已关闭的请求会在 workspace session 校验处失败，因而无法在 B 打开后把 A 的
    /// metadata、summary 或 response ownership 写入 B 的全局状态。
    pub fn record_http_response(
        &self,
        access: ResponseAccess,
        meta: &ResponseMeta,
    ) -> Result<ResponseSummary> {
        self.with_workspace(access.workspace_id, access.session_id, |_| {
            self.response_scopes
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(meta.id, ResponseScope::Workspace(access));
            self.meta
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((
                    meta.id,
                    meta.size_bytes,
                    meta.timing,
                    meta.content_type.clone(),
                ));
            self.summarize_http_unchecked(meta)
        })
    }

    /// 请求完成或出错后清理注册表条目。
    pub fn finish_inflight(&self, id: Uuid) {
        let mut guard = self
            .in_flight
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.remove(&id);
    }

    /// 生成合成响应并流式落盘（Spike S2）。
    ///
    /// 刻意分块写入，模拟真实的 reqwest 响应流：内存里从不持有完整 body。
    pub fn make_synthetic(&self, target_bytes: usize) -> Result<ResponseSummary> {
        self.make_synthetic_with_writer(target_bytes, Self::write_synthetic_payload)
    }

    /// 将 runtime 捕获、锁外写入与二次发布校验拆开，测试可以注入可阻塞 writer 来
    /// 证明工作区切换不会等待 S2 的大 body 生成。
    fn make_synthetic_with_writer(
        &self,
        target_bytes: usize,
        writer: impl FnOnce(&Spool, Uuid, usize) -> Result<u64>,
    ) -> Result<ResponseSummary> {
        let runtime = self.capture_synthetic_runtime();
        let id = Uuid::new_v4();
        let t0 = Instant::now();
        let size = writer(runtime.runtime.spool(), id, target_bytes)?;
        let gen_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let timing = Timing {
            total_ms: gen_ms,
            download_ms: gen_ms,
            ..Default::default()
        };

        let result = self.with_current_synthetic_runtime(&runtime, |_| {
            self.response_scopes
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(id, ResponseScope::Synthetic);
            self.meta
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((id, size, timing, Some("application/json".into())));
            Ok(self.build_summary(id, size, timing, Some("application/json".into())))
        });
        if result.is_err() {
            // runtime 已失效时 body 绝不能留给后续 session 使用；session 唯一目录使
            // 这里的清理不会触碰新 runtime。
            let _ = runtime.runtime.spool().remove(id);
        }
        result
    }

    fn write_synthetic_payload(spool: &Spool, id: Uuid, target_bytes: usize) -> Result<u64> {
        // 生成 1MB 的模板块，然后重复写到目标体积。
        // 这样内存峰值是 1MB 而不是 100MB —— 否则量测本身就先把内存打爆了。
        const BLOCK: usize = 1024 * 1024;
        let head = b"{\"code\":0,\"message\":\"ok\",\"data\":{\"list\":[".to_vec();
        let tail = b"]}}".to_vec();
        let block = {
            // synth_json_payload 产出的是完整 JSON，这里取它 list 内部的元素片段复用
            let full = synth_json_payload(BLOCK);
            let s = String::from_utf8_lossy(&full).to_string();
            let start = s.find('[').map(|i| i + 1).unwrap_or(0);
            let end = s.rfind("]}}").unwrap_or(s.len());
            s.as_bytes()[start..end].to_vec()
        };

        // 真正的流式：迭代器只反复交出同一个 1MB 块的**引用**，
        // 内存峰值恒定在一个块。
        let n_blocks = target_bytes / block.len() + 1;
        let comma: &[u8] = b",";
        let block_ref: &[u8] = block.as_slice();
        let chunks = std::iter::once(head.as_slice())
            .chain((0..n_blocks).flat_map(move |i| {
                let sep: Option<&[u8]> = if i == 0 { None } else { Some(comma) };
                sep.into_iter().chain(std::iter::once(block_ref))
            }))
            .chain(std::iter::once(tail.as_slice()));
        Ok(spool.write_stream(id, chunks)?)
    }

    fn summary_unchecked(&self, id: Uuid) -> Result<ResponseSummary> {
        if let Some(mut summary) = self
            .semantic_meta
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&id)
            .cloned()
        {
            let index = self.store.index(id);
            summary.indexed = index.is_some();
            summary.total_nodes = index.as_ref().map(|item| item.total_nodes());
            summary.index_bytes = index.as_ref().map(|item| item.estimated_bytes());
            return Ok(summary);
        }
        let m = self
            .meta
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_, size, timing, ct) = m
            .iter()
            .find(|(x, ..)| *x == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("未知的响应 id"))?;
        Ok(self.build_summary(id, size, timing, ct))
    }

    pub fn response_summary(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        response_id: Uuid,
    ) -> Result<ResponseSummary> {
        self.response_summary_for_access(
            Self::response_access(workspace_id, session_id),
            response_id,
        )
    }

    /// 后台请求只能通过它所属的 [`ResponseAccess`] 重新读取摘要；session 已失效时
    /// 不返回旧摘要，也不允许继续发出可消费的 `request:done`。
    pub fn response_summary_for_access(
        &self,
        access: ResponseAccess,
        response_id: Uuid,
    ) -> Result<ResponseSummary> {
        self.with_response_summary_for_access(access, response_id, Ok)
    }

    /// 在 response scope 仍有效的同一临界区内读取摘要并执行短操作。
    ///
    /// `request:done` 用它把摘要构造与事件入队绑在一起：`close_workspace` 要么先
    /// 使 access 失效、整个操作失败，要么只能等事件已经入队后再清理状态。
    pub fn with_response_summary_for_access<T>(
        &self,
        access: ResponseAccess,
        response_id: Uuid,
        operation: impl FnOnce(ResponseSummary) -> Result<T>,
    ) -> Result<T> {
        self.with_response(access, response_id, |_| {
            operation(self.summary_unchecked(response_id)?)
        })
    }

    fn build_summary(
        &self,
        id: Uuid,
        size: u64,
        timing: Timing,
        content_type: Option<String>,
    ) -> ResponseSummary {
        let index = self.store.index(id);
        ResponseSummary {
            content_type,
            indexed: index.is_some(),
            total_nodes: index.as_ref().map(|item| item.total_nodes()),
            index_bytes: index.as_ref().map(|item| item.estimated_bytes()),
            // 合成响应没有 HTTP 语义，status/headers 保持 None —— 前端据此
            // 显示「不可用」，而不是编一个 200 出来。
            ..ResponseSummary::without_http(id, size, timing)
        }
    }

    /// 带 HTTP 语义的完整摘要：把 `ResponseMeta` 的投影与当前索引状态合并。
    ///
    /// 为什么不直接用 `build_summary`：它走 `without_http`，会丢掉 status/headers。
    /// 为什么不直接用 `ResponseSummary::from_meta`：它把 `indexed` 写死为 false。
    /// 请求完成路径两者都要，所以在这里合并。
    fn summarize_http_unchecked(&self, meta: &ResponseMeta) -> Result<ResponseSummary> {
        let index = self.store.index(meta.id);
        let mut summary = ResponseSummary::from_meta(meta);
        summary.indexed = index.is_some();
        summary.total_nodes = index.as_ref().map(|item| item.total_nodes());
        summary.index_bytes = index.as_ref().map(|item| item.estimated_bytes());
        self.semantic_meta
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(meta.id, summary.clone());
        Ok(summary)
    }

    /// 读取历史记录，并将其中的 response body 恢复到当前 session 的唯一 spool。
    ///
    /// 第一段短临界区只读取 YAML/获取 history body 路径并克隆 session spool；body 的
    /// 大文件导入完全在锁外进行。导入完成后必须再次校验 session，旧任务最多写入其
    /// 已失效的私有目录，绝不会向新 runtime 注册 summary 或 metadata。
    pub fn read_history(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        history_id: Uuid,
    ) -> Result<crate::workspace::HistoryRecord> {
        self.read_history_with_restorer(
            workspace_id,
            session_id,
            history_id,
            Self::restore_history_response_into,
        )
    }

    /// 供并发回归注入可阻塞导入器；生产路径使用 [`Self::restore_history_response_into`]。
    fn read_history_with_restorer(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        history_id: Uuid,
        restorer: impl FnOnce(&Spool, ResponseSummary, &std::path::Path) -> Result<ResponseSummary>,
    ) -> Result<crate::workspace::HistoryRecord> {
        let access = Self::response_access(workspace_id, session_id);
        let (mut record, body, runtime) =
            self.with_workspace_runtime(workspace_id, session_id, |repository, runtime| {
                let (record, body) = repository.read_history(history_id)?;
                Ok((record, body, Arc::clone(runtime)))
            })?;

        if let Some(response) = record.response.take() {
            let body = body.ok_or_else(|| anyhow::anyhow!("历史响应体不存在：{history_id}"))?;
            // History records only retain the public projection. Recreate the minimum internal
            // metadata needed for spool restoration, then immediately project it back before the
            // command can serialize the record.
            let response = restorer(
                runtime.spool(),
                response.into_internal_for_history_restore(),
                &body,
            )?;
            let response_id = response.id;
            match self.publish_history_response(access, response) {
                Ok(response) => record.response = Some(PublicResponseSummary::from(&response)),
                Err(error) => {
                    // 只会清理已失效 session 的私有 spool，不能误删新 runtime 的 body。
                    let _ = runtime.spool().remove(response_id);
                    return Err(error);
                }
            }
        }

        // 没有 response body 的失败历史也需要二次校验，避免已经切换工作区的调用方得到
        // 一条看似仍可操作的旧 session 记录。
        self.with_active_response_access(access, || Ok(()))?;
        Ok(record)
    }

    /// 在 session 仍有效时发布 history response 的可见范围和摘要元数据。
    fn publish_history_response(
        &self,
        access: ResponseAccess,
        summary: ResponseSummary,
    ) -> Result<ResponseSummary> {
        self.with_active_response_access(access, || {
            self.response_scopes
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(summary.id, ResponseScope::Workspace(access));
            self.meta
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((
                    summary.id,
                    summary.size_bytes,
                    summary.timing,
                    summary.content_type.clone(),
                ));
            self.semantic_meta
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(summary.id, summary.clone());
            Ok(summary)
        })
    }

    /// 把一个历史响应体原子恢复到调用方先前捕获的 session spool。
    ///
    /// 这个函数不访问 `AppState`，因此 `copy_from` 可以在所有 workspace/spool 锁外执行。
    fn restore_history_response_into(
        spool: &Spool,
        mut summary: ResponseSummary,
        body: &std::path::Path,
    ) -> Result<ResponseSummary> {
        // `copy_from` 会验证已有 body 的长度；不能用 `exists()` 裸短路，否则损坏或
        // 同 ID 的错误文件会被当作已恢复响应直接发布。
        spool
            .copy_from(summary.id, body)
            .with_context(|| format!("恢复历史响应体失败：{}", body.display()))?;
        summary.indexed = false;
        summary.total_nodes = None;
        summary.index_bytes = None;
        Ok(summary)
    }

    /// 直接对不可变 spool 文件做只读 mmap 并构建 SpanIndex。
    ///
    /// mmap 只建立虚拟地址映射，不把完整 body 复制到 Rust 堆；索引构建结束后立即解除映射，
    /// 常驻内存只剩固定 32 字节/节点的元数据和去重后的对象键。
    ///
    /// 此函数故意不访问 `AppState`：调用方必须在短临界区内先验证响应归属并克隆
    /// `ResponseRuntime` lease，再在锁外调用它。这样大型 JSON 的解析不会阻塞工作区
    /// 关闭或切换，且旧 session spool 会持续存活至解析结束。
    fn parse_response_index(spool: &Spool, id: Uuid) -> Result<SpanIndex> {
        let path = spool.path_for(id);
        let file = File::open(&path)
            .with_context(|| format!("打开响应 spool 失败：{}", path.display()))?;
        // SAFETY: spool 文件写完后通过原子 rename 发布，发布后只读且不会原地改写；
        // 映射的生命周期不超过 file 和本函数，SpanIndex 返回时不借用映射内容。
        let mapped = unsafe { MmapOptions::new().map(&file) }
            .with_context(|| format!("映射响应 spool 失败：{}", path.display()))?;
        SpanIndex::parse(&mapped).map_err(|error| anyhow::anyhow!(error))
    }

    /// 普通响应的两阶段索引流程。
    ///
    /// 第一阶段只在已验证的 session 内克隆 spool；解析发生在所有 workspace/spool 锁
    /// 之外。解析后必须再次校验 access 和 response scope，才能发布 index，避免旧
    /// workspace 的慢解析在新 runtime 中写入同名 response id。
    fn index_response_for_access_with_parser(
        &self,
        access: ResponseAccess,
        response_id: Uuid,
        parse: impl FnOnce(&Spool, Uuid) -> Result<SpanIndex>,
    ) -> Result<ResponseSummary> {
        let runtime =
            self.with_response_runtime(access, response_id, |runtime| Ok(Arc::clone(runtime)))?;
        let index = parse(runtime.spool(), response_id)?;
        self.with_response(access, response_id, |_| {
            self.store.insert_index(response_id, index);
            self.summary_unchecked(response_id)
        })
    }

    /// 合成响应与普通响应拥有相同的短锁/锁外解析/二次验证边界，只是其可见范围不
    /// 依赖 workspace session。
    fn index_synthetic_response_with_parser(
        &self,
        response_id: Uuid,
        parse: impl FnOnce(&Spool, Uuid) -> Result<SpanIndex>,
    ) -> Result<ResponseSummary> {
        let runtime =
            self.with_synthetic_runtime(response_id, |runtime| Ok(Arc::clone(runtime)))?;
        let index = parse(runtime.spool(), response_id)?;
        self.with_synthetic_response(response_id, |_| {
            self.store.insert_index(response_id, index);
            self.summary_unchecked(response_id)
        })
    }

    pub fn index_response(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        response_id: Uuid,
    ) -> Result<ResponseSummary> {
        let access = Self::response_access(workspace_id, session_id);
        self.index_response_for_access(access, response_id)
    }

    /// 后台 HTTP 完成路径只接受 RequestLease 携带的 access 建索引。
    pub fn index_response_for_access(
        &self,
        access: ResponseAccess,
        response_id: Uuid,
    ) -> Result<ResponseSummary> {
        self.index_response_for_access_with_parser(access, response_id, Self::parse_response_index)
    }

    pub fn index_synthetic_response(&self, response_id: Uuid) -> Result<ResponseSummary> {
        self.index_synthetic_response_with_parser(response_id, Self::parse_response_index)
    }

    /// 读取一页节点，并只为这一页的标量按 span 拉取最多 4KB 预览。
    fn tree_page_in_spool(
        &self,
        spool: &Spool,
        id: Uuid,
        path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TreeNode>> {
        let index = self
            .store
            .index(id)
            .ok_or_else(|| anyhow::anyhow!("该响应尚未建索引，请先调用 index_response"))?;
        index
            .children(path, offset, limit)
            .into_iter()
            .map(|mut indexed| {
                if let Some(span) = indexed.value_span {
                    let requested = span.len().min(PREVIEW_READ_BYTES);
                    let bytes = spool.read_range(id, span.start, requested)?;
                    let complete = bytes.len() as u64 == span.len();
                    indexed.node.preview =
                        Some(scalar_preview(indexed.node.kind, &bytes, complete));
                }
                Ok(indexed.node)
            })
            .collect()
    }

    pub fn response_tree_page(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        response_id: Uuid,
        path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TreeNode>> {
        let access = Self::response_access(workspace_id, session_id);
        self.with_response(access, response_id, |spool| {
            self.tree_page_in_spool(spool, response_id, path, offset, limit)
        })
    }

    pub fn synthetic_tree_page(
        &self,
        response_id: Uuid,
        path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TreeNode>> {
        self.with_synthetic_response(response_id, |spool| {
            self.tree_page_in_spool(spool, response_id, path, offset, limit)
        })
    }

    pub fn response_size(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        response_id: Uuid,
    ) -> Result<u64> {
        let access = Self::response_access(workspace_id, session_id);
        self.with_response(access, response_id, |spool| Ok(spool.size_of(response_id)?))
    }

    pub fn response_range(
        &self,
        workspace_id: Uuid,
        session_id: Uuid,
        response_id: Uuid,
        start: u64,
        len: u64,
    ) -> Result<Vec<u8>> {
        let access = Self::response_access(workspace_id, session_id);
        self.with_response(access, response_id, |spool| {
            Ok(spool.read_range(response_id, start, len)?)
        })
    }

    pub fn synthetic_response_size(&self, response_id: Uuid) -> Result<u64> {
        self.with_synthetic_response(response_id, |spool| Ok(spool.size_of(response_id)?))
    }

    pub fn synthetic_response_range(
        &self,
        response_id: Uuid,
        start: u64,
        len: u64,
    ) -> Result<Vec<u8>> {
        self.with_synthetic_response(response_id, |spool| {
            Ok(spool.read_range(response_id, start, len)?)
        })
    }
}

/// Spike 报告输出目录。
///
/// `APOGEE_SPIKE_RESULTS_DIR` 优先：外部量测台（如 `scripts/measure-s3-windows.ps1`）
/// 需要**确定**报告落在哪，不能依赖 cwd 推断。release 二进制从 `target/release`
/// 启动时，下面那套「cwd 的上一级」启发式会落到 `target/release/spike-results`，
/// 量测台就找不到文件了 —— 这属于「起点和终点都不能靠推断」的同一类问题。
///
/// 没有该环境变量时才退回按 cwd 猜：`tauri dev` 的 cwd 是 src-tauri，
/// 所以先看 `../spikes/results`，再退回 cwd 下的同名目录。
pub fn spike_results_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("APOGEE_SPIKE_RESULTS_DIR") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for cand in [
        cwd.join("..").join("spikes").join("results"),
        cwd.join("spikes").join("results"),
    ] {
        if let Some(parent) = cand.parent() {
            if parent.exists() {
                return cand;
            }
        }
    }
    cwd.join("spike-results")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_workspace_replaces_an_orphaned_session() {
        let root = std::env::temp_dir().join(format!("apogee-session-test-{}", Uuid::new_v4()));
        let first_root = root.join("first");
        let second_root = root.join("second");
        let state = AppState::new().unwrap();

        let first = WorkspaceRepository::create(&first_root, "First".into()).unwrap();
        let (first_workspace, first_session) = state
            .open_workspace(first, root.join("runtime-first"))
            .unwrap();
        assert!(state
            .runtime
            .read()
            .unwrap()
            .spool()
            .path_for(Uuid::new_v4())
            .starts_with(
                root.join("runtime-first")
                    .join("responses")
                    .join(first_session.to_string())
            ));
        let mut cancellation = state.register_inflight(Uuid::new_v4(), None);
        state
            .with_workspace(first_workspace, first_session, |repository| {
                repository.snapshot()
            })
            .unwrap();

        // 前端重载后可能残留孤儿会话；再次 open 应替换旧会话而不是报
        // WORKSPACE_ALREADY_OPEN，否则用户会卡死在启动页无法进入任何工作区。
        let second = WorkspaceRepository::create(&second_root, "Second".into()).unwrap();
        let (second_workspace, second_session) = state
            .open_workspace(second, root.join("runtime-second"))
            .unwrap();
        assert_ne!(second_session, first_session);
        // 替换时取消旧会话的 in-flight 请求，与 close_workspace 语义一致。
        assert_eq!(cancellation.try_recv(), Ok(()));
        assert!(state
            .runtime
            .read()
            .unwrap()
            .spool()
            .path_for(Uuid::new_v4())
            .starts_with(
                root.join("runtime-second")
                    .join("responses")
                    .join(second_session.to_string())
            ));

        // 旧会话已失效，新会话可以正常使用。
        let error = state
            .with_workspace(first_workspace, first_session, |repository| {
                repository.snapshot()
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "WORKSPACE_SESSION_EXPIRED");
        state
            .with_workspace(second_workspace, second_session, |repository| {
                repository.snapshot()
            })
            .unwrap();
        state
            .close_workspace(second_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn closed_session_cannot_persist_into_newly_opened_workspace() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let root =
            std::env::temp_dir().join(format!("apogee-session-write-test-{}", Uuid::new_v4()));
        let first_root = root.join("first");
        let second_root = root.join("second");
        let state = AppState::new().unwrap();

        let first = WorkspaceRepository::create(&first_root, "First".into()).unwrap();
        let collection_id = first
            .create_collection("API".into())
            .unwrap()
            .nodes
            .into_iter()
            .find(|node| node.name == "API")
            .expect("新建集合必须出现在快照中")
            .id;
        let original = first
            .create_request(collection_id, "Original".into())
            .unwrap();
        let mut stale_spec = original.spec.clone();
        stale_spec.url = "https://must-not-be-written.invalid".into();
        let request_id = original.id;
        let original_name = original.name.clone();
        let original_url = original.spec.url.clone();
        let (first_workspace, first_session) = state
            .open_workspace(first, root.join("runtime-first"))
            .unwrap();

        state
            .close_workspace(first_workspace, first_session)
            .unwrap();
        let second = WorkspaceRepository::create(&second_root, "Second".into()).unwrap();
        let (second_workspace, second_session) = state
            .open_workspace(second, root.join("runtime-second"))
            .unwrap();

        let stale_write_entered = AtomicBool::new(false);
        let error = state
            .with_workspace(first_workspace, first_session, |repository| {
                stale_write_entered.store(true, Ordering::SeqCst);
                repository.save_request(request_id, "Stale write".into(), stale_spec)
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "WORKSPACE_SESSION_EXPIRED");
        assert!(!stale_write_entered.load(Ordering::SeqCst));

        let persisted = WorkspaceRepository::open(&first_root)
            .unwrap()
            .read_request(request_id)
            .unwrap();
        assert_eq!(persisted.name, original_name);
        assert_eq!(persisted.spec.url, original_url);
        state
            .with_workspace(second_workspace, second_session, |repository| {
                repository.snapshot()
            })
            .unwrap();
        state
            .close_workspace(second_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn close_waits_for_active_workspace_operation_before_invalidating_session() {
        use std::sync::{
            mpsc::{self, TryRecvError},
            Arc, Barrier,
        };
        use std::thread;

        let root =
            std::env::temp_dir().join(format!("apogee-session-barrier-test-{}", Uuid::new_v4()));
        let first_root = root.join("first");
        let second_root = root.join("second");
        let state = Arc::new(AppState::new().unwrap());

        let first = WorkspaceRepository::create(&first_root, "First".into()).unwrap();
        let (first_workspace, first_session) = state
            .open_workspace(first, root.join("runtime-first"))
            .unwrap();
        let operation_started = Arc::new(Barrier::new(2));
        let release_operation = Arc::new(Barrier::new(2));
        let worker_state = Arc::clone(&state);
        let worker_started = Arc::clone(&operation_started);
        let worker_release = Arc::clone(&release_operation);
        let worker = thread::spawn(move || {
            worker_state.with_workspace(first_workspace, first_session, |repository| {
                worker_started.wait();
                worker_release.wait();
                repository.snapshot()
            })
        });

        operation_started.wait();
        assert!(
            state.workspace.try_write().is_err(),
            "活动工作区操作必须在整个闭包期间持有读锁"
        );

        let (close_started_tx, close_started_rx) = mpsc::channel();
        let (close_completed_tx, close_completed_rx) = mpsc::channel();
        let closer_state = Arc::clone(&state);
        let closer = thread::spawn(move || {
            close_started_tx.send(()).unwrap();
            let result = closer_state
                .close_workspace(first_workspace, first_session)
                .map_err(|error| error.to_string());
            close_completed_tx.send(result).unwrap();
        });

        close_started_rx.recv().unwrap();
        assert_eq!(close_completed_rx.try_recv(), Err(TryRecvError::Empty));

        release_operation.wait();
        worker.join().unwrap().unwrap();
        close_completed_rx.recv().unwrap().unwrap();
        closer.join().unwrap();

        let second = WorkspaceRepository::create(&second_root, "Second".into()).unwrap();
        let (second_workspace, second_session) = state
            .open_workspace(second, root.join("runtime-second"))
            .unwrap();
        let stale_error = state
            .with_workspace(first_workspace, first_session, |repository| {
                repository.snapshot()
            })
            .unwrap_err();
        assert_eq!(stale_error.to_string(), "WORKSPACE_SESSION_EXPIRED");
        state
            .with_workspace(second_workspace, second_session, |repository| {
                repository.snapshot()
            })
            .unwrap();
        state
            .close_workspace(second_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn request_lease_binds_spool_to_session_and_close_cancels_it() {
        let root =
            std::env::temp_dir().join(format!("apogee-request-lease-test-{}", Uuid::new_v4()));
        let first_root = root.join("first");
        let second_root = root.join("second");
        let state = AppState::new().unwrap();

        let first = WorkspaceRepository::create(&first_root, "First".into()).unwrap();
        let (first_workspace, first_session) = state
            .open_workspace(first, root.join("runtime-first"))
            .unwrap();
        let request_id = Uuid::new_v4();
        let first_lease = state
            .begin_request(
                first_workspace,
                first_session,
                request_id,
                Some("first-tab".into()),
            )
            .unwrap();
        let (first_runtime, mut first_cancellation, _) = first_lease.into_execution_parts();
        assert!(first_runtime.spool().path_for(request_id).starts_with(
            root.join("runtime-first")
                .join("responses")
                .join(first_session.to_string())
        ));

        state
            .close_workspace(first_workspace, first_session)
            .unwrap();
        assert_eq!(first_cancellation.try_recv(), Ok(()));
        drop(first_runtime);

        let second = WorkspaceRepository::create(&second_root, "Second".into()).unwrap();
        let (second_workspace, second_session) = state
            .open_workspace(second, root.join("runtime-second"))
            .unwrap();
        let stale_error =
            match state.begin_request(first_workspace, first_session, Uuid::new_v4(), None) {
                Ok(_) => panic!("过期 session 不得获得 request lease"),
                Err(error) => error,
            };
        assert_eq!(stale_error.to_string(), "WORKSPACE_SESSION_EXPIRED");

        let second_id = Uuid::new_v4();
        let second_lease = state
            .begin_request(second_workspace, second_session, second_id, None)
            .unwrap();
        let (second_runtime, mut second_cancellation, _) = second_lease.into_execution_parts();
        assert!(second_runtime.spool().path_for(second_id).starts_with(
            root.join("runtime-second")
                .join("responses")
                .join(second_session.to_string())
        ));
        state
            .close_workspace(second_workspace, second_session)
            .unwrap();
        assert_eq!(second_cancellation.try_recv(), Ok(()));
        drop(second_runtime);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_request_runtime_is_reclaimed_after_it_finishes_without_touching_new_session() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let root = std::env::temp_dir().join(format!(
            "apogee-runtime-reclamation-test-{}",
            Uuid::new_v4()
        ));
        let workspace_root = root.join("workspace");
        let runtime_root = root.join("runtime");
        let state = Arc::new(AppState::new().unwrap());
        let first = WorkspaceRepository::create(&workspace_root, "First".into()).unwrap();
        let (workspace_id, first_session) = state.open_workspace(first, &runtime_root).unwrap();
        let first_response_id = Uuid::new_v4();
        let first_lease = state
            .begin_request(workspace_id, first_session, first_response_id, None)
            .unwrap();
        let (first_runtime, mut first_cancellation, _) = first_lease.into_execution_parts();
        let first_spool_root = first_runtime
            .spool()
            .path_for(first_response_id)
            .parent()
            .expect("response spool path 必须有父目录")
            .to_path_buf();
        let writer_started = Arc::new(Barrier::new(2));
        let release_writer = Arc::new(Barrier::new(2));
        let worker_runtime = Arc::clone(&first_runtime);
        let worker_started = Arc::clone(&writer_started);
        let worker_release = Arc::clone(&release_writer);
        let worker = thread::spawn(move || {
            let payload: &[u8] = br#"{"session":"first"}"#;
            worker_started.wait();
            worker_release.wait();
            worker_runtime
                .spool()
                .write_stream(first_response_id, [payload])
        });

        writer_started.wait();
        // 模拟已经拿到 runtime 的旧网络任务：close/open 不应等它，也不能提前删目录。
        drop(first_runtime);
        state.close_workspace(workspace_id, first_session).unwrap();
        assert_eq!(first_cancellation.try_recv(), Ok(()));
        let reopened = WorkspaceRepository::open(&workspace_root).unwrap();
        let (reopened_workspace, second_session) =
            state.open_workspace(reopened, &runtime_root).unwrap();
        assert_eq!(reopened_workspace, workspace_id);
        assert_ne!(second_session, first_session);

        let second_response_id = Uuid::new_v4();
        let second_payload = br#"{"session":"second"}"#;
        let second_spool_root = state
            .runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .spool()
            .path_for(second_response_id)
            .parent()
            .expect("response spool path 必须有父目录")
            .to_path_buf();
        state
            .with_workspace_runtime(reopened_workspace, second_session, |_, runtime| {
                runtime
                    .spool()
                    .write_stream(second_response_id, [&second_payload[..]])?;
                Ok(())
            })
            .unwrap();
        assert!(
            first_spool_root.exists(),
            "仍在收尾的旧任务必须保留自己的 session spool"
        );
        assert_eq!(
            std::fs::read(second_spool_root.join(format!("{second_response_id}.bin"))).unwrap(),
            second_payload
        );

        release_writer.wait();
        worker.join().unwrap().unwrap();
        assert!(
            !first_spool_root.exists(),
            "最后一个旧 runtime lease 释放后必须回收 session spool"
        );
        assert!(
            second_spool_root.exists(),
            "旧 runtime 回收不得删除当前 session spool"
        );
        assert_eq!(
            std::fs::read(second_spool_root.join(format!("{second_response_id}.bin"))).unwrap(),
            second_payload
        );

        state
            .close_workspace(reopened_workspace, second_session)
            .unwrap();
        assert!(
            !second_spool_root.exists(),
            "没有外部 lease 的关闭 session 应立即回收 spool"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_idle_session_switches_reclaim_all_retired_spools() {
        let root = std::env::temp_dir().join(format!(
            "apogee-runtime-reclamation-loop-test-{}",
            Uuid::new_v4()
        ));
        let workspace_root = root.join("workspace");
        let runtime_root = root.join("runtime");
        let state = AppState::new().unwrap();
        let repository = WorkspaceRepository::create(&workspace_root, "First".into()).unwrap();
        let (workspace_id, mut session_id) =
            state.open_workspace(repository, &runtime_root).unwrap();
        let mut retired_spools = Vec::new();

        for _ in 0..3 {
            let spool_root = state
                .runtime
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .spool()
                .path_for(Uuid::new_v4())
                .parent()
                .expect("response spool path 必须有父目录")
                .to_path_buf();
            retired_spools.push(spool_root.clone());
            state.close_workspace(workspace_id, session_id).unwrap();
            assert!(
                !spool_root.exists(),
                "空闲 session 关闭后不得保留 response spool"
            );

            let reopened = WorkspaceRepository::open(&workspace_root).unwrap();
            let (reopened_workspace, next_session_id) =
                state.open_workspace(reopened, &runtime_root).unwrap();
            assert_eq!(reopened_workspace, workspace_id);
            session_id = next_session_id;
        }

        let active_spool = state
            .runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .spool()
            .path_for(Uuid::new_v4())
            .parent()
            .expect("response spool path 必须有父目录")
            .to_path_buf();
        state.close_workspace(workspace_id, session_id).unwrap();
        assert!(retired_spools.iter().all(|path| !path.exists()));
        assert!(!active_spool.exists());
        assert!(
            std::fs::read_dir(runtime_root.join("responses"))
                .unwrap()
                .next()
                .is_none(),
            "重复 session 切换后 responses 目录不得累积已退休 spool"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_spool_operation_binds_path_to_active_session() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let root =
            std::env::temp_dir().join(format!("apogee-history-spool-test-{}", Uuid::new_v4()));
        let first_root = root.join("first");
        let second_root = root.join("second");
        let state = AppState::new().unwrap();

        let first = WorkspaceRepository::create(&first_root, "First".into()).unwrap();
        let (first_workspace, first_session) = state
            .open_workspace(first, root.join("runtime-first"))
            .unwrap();
        let first_response_id = Uuid::new_v4();
        let first_body = state
            .with_workspace_runtime(first_workspace, first_session, |_, runtime| {
                Ok(runtime.spool().path_for(first_response_id))
            })
            .unwrap();
        assert!(first_body.starts_with(
            root.join("runtime-first")
                .join("responses")
                .join(first_session.to_string())
        ));

        state
            .close_workspace(first_workspace, first_session)
            .unwrap();
        let second = WorkspaceRepository::create(&second_root, "Second".into()).unwrap();
        let (second_workspace, second_session) = state
            .open_workspace(second, root.join("runtime-second"))
            .unwrap();

        let stale_operation_entered = AtomicBool::new(false);
        let stale_error = state
            .with_workspace_runtime(first_workspace, first_session, |_, _| {
                stale_operation_entered.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err();
        assert_eq!(stale_error.to_string(), "WORKSPACE_SESSION_EXPIRED");
        assert!(!stale_operation_entered.load(Ordering::SeqCst));

        let second_body = state
            .with_workspace_runtime(second_workspace, second_session, |_, runtime| {
                Ok(runtime.spool().path_for(Uuid::new_v4()))
            })
            .unwrap();
        assert!(second_body.starts_with(
            root.join("runtime-second")
                .join("responses")
                .join(second_session.to_string())
        ));
        state
            .close_workspace(second_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_read_restores_body_only_into_its_active_session_spool() {
        use apogee_core::request::{AuthConfig, Body, RequestSettings, RequestSpec};

        let root = std::env::temp_dir().join(format!(
            "apogee-history-read-session-test-{}",
            Uuid::new_v4()
        ));
        let first_root = root.join("first");
        let second_root = root.join("second");
        let first = WorkspaceRepository::create(&first_root, "First".into()).unwrap();
        let response_id = Uuid::new_v4();
        let payload = br#"{"history":true}"#;
        let source = root.join("history-source.body");
        std::fs::write(&source, payload).unwrap();
        first
            .record_history(
                1234,
                PublicRequestPreview::from(&RequestSpec {
                    method: "GET".into(),
                    url: "https://history.example.test".into(),
                    params: Vec::new(),
                    path_params: Vec::new(),
                    headers: Vec::new(),
                    body: Body::None,
                    auth: AuthConfig::Inherit,
                    settings: RequestSettings::default(),
                }),
                PublicResponseSummary::from(&ResponseSummary::without_http(
                    response_id,
                    payload.len() as u64,
                    Timing::default(),
                )),
                &source,
                None,
                None,
            )
            .unwrap();

        let state = AppState::new().unwrap();
        let (first_workspace, first_session) = state
            .open_workspace(first, root.join("runtime-first"))
            .unwrap();
        let record = state
            .read_history(first_workspace, first_session, response_id)
            .unwrap();
        assert_eq!(record.id, response_id);
        assert_eq!(
            record.response.expect("成功历史必须恢复响应").id,
            response_id
        );
        let first_runtime_body = root
            .join("runtime-first")
            .join("responses")
            .join(first_session.to_string())
            .join(format!("{response_id}.bin"));
        assert_eq!(std::fs::read(&first_runtime_body).unwrap(), payload);

        // 已恢复的 response id 再次读取时也必须验证现有 spool body，不能用 exists()
        // 直接跳过，否则错误文件会重新作为历史响应发布。
        std::fs::write(&first_runtime_body, b"stale").unwrap();
        assert!(state
            .read_history(first_workspace, first_session, response_id)
            .is_err());
        assert_eq!(std::fs::read(&first_runtime_body).unwrap(), b"stale");

        state
            .close_workspace(first_workspace, first_session)
            .unwrap();
        let second = WorkspaceRepository::create(&second_root, "Second".into()).unwrap();
        let (second_workspace, second_session) = state
            .open_workspace(second, root.join("runtime-second"))
            .unwrap();
        let stale_error = state
            .read_history(first_workspace, first_session, response_id)
            .unwrap_err();
        assert_eq!(stale_error.to_string(), "WORKSPACE_SESSION_EXPIRED");
        let second_runtime_body = root
            .join("runtime-second")
            .join("responses")
            .join(second_session.to_string())
            .join(format!("{response_id}.bin"));
        assert!(
            !second_runtime_body.exists(),
            "过期 session 不得把旧历史响应恢复到新工作区 spool"
        );
        state
            .close_workspace(second_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_restore_releases_runtime_locks_and_revalidates_before_publish() {
        use apogee_core::request::{AuthConfig, Body, RequestSettings, RequestSpec};
        use std::sync::{Arc, Barrier};
        use std::thread;

        let root = std::env::temp_dir().join(format!(
            "apogee-history-restore-runtime-test-{}",
            Uuid::new_v4()
        ));
        let workspace_root = root.join("workspace");
        let runtime_root = root.join("runtime");
        let repository = WorkspaceRepository::create(&workspace_root, "First".into()).unwrap();
        let response_id = Uuid::new_v4();
        let payload = br#"{"history":true}"#;
        let source = root.join("history-source.body");
        std::fs::write(&source, payload).unwrap();
        repository
            .record_history(
                1234,
                PublicRequestPreview::from(&RequestSpec {
                    method: "GET".into(),
                    url: "https://history.example.test".into(),
                    params: Vec::new(),
                    path_params: Vec::new(),
                    headers: Vec::new(),
                    body: Body::None,
                    auth: AuthConfig::Inherit,
                    settings: RequestSettings::default(),
                }),
                PublicResponseSummary::from(&ResponseSummary::without_http(
                    response_id,
                    payload.len() as u64,
                    Timing::default(),
                )),
                &source,
                None,
                None,
            )
            .unwrap();

        let state = Arc::new(AppState::new().unwrap());
        let (workspace_id, first_session) =
            state.open_workspace(repository, &runtime_root).unwrap();
        let first_spool_root = runtime_root
            .join("responses")
            .join(first_session.to_string());
        let copy_started = Arc::new(Barrier::new(2));
        let release_copy = Arc::new(Barrier::new(2));
        let worker_state = Arc::clone(&state);
        let worker_started = Arc::clone(&copy_started);
        let worker_release = Arc::clone(&release_copy);
        let worker = thread::spawn(move || {
            worker_state.read_history_with_restorer(
                workspace_id,
                first_session,
                response_id,
                move |spool, summary, body| {
                    worker_started.wait();
                    worker_release.wait();
                    AppState::restore_history_response_into(spool, summary, body)
                },
            )
        });

        copy_started.wait();
        drop(
            state
                .workspace
                .try_write()
                .expect("history body 导入期间不得持有 workspace 读锁"),
        );
        drop(
            state
                .runtime
                .try_write()
                .expect("history body 导入期间不得持有 spool 读锁"),
        );

        state.close_workspace(workspace_id, first_session).unwrap();
        let reopened = WorkspaceRepository::open(&workspace_root).unwrap();
        let (reopened_workspace, second_session) =
            state.open_workspace(reopened, &runtime_root).unwrap();
        assert_eq!(reopened_workspace, workspace_id);
        assert_ne!(second_session, first_session);
        let second_runtime_body = runtime_root
            .join("responses")
            .join(second_session.to_string())
            .join(format!("{response_id}.bin"));

        release_copy.wait();
        let stale_error = worker.join().unwrap().unwrap_err();
        assert_eq!(stale_error.to_string(), "WORKSPACE_SESSION_EXPIRED");
        assert!(
            !first_spool_root.exists(),
            "history runtime lease 释放后必须回收旧 session spool"
        );
        assert!(
            !second_runtime_body.exists(),
            "旧 session 的锁外 history 导入不得写入重开后的 session spool"
        );
        assert!(
            state
                .response_scopes
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "旧 session 的 history 不得向新 runtime 发布 response scope"
        );
        state
            .close_workspace(reopened_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn synthetic_generation_releases_runtime_locks_and_does_not_publish_after_switch() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let root =
            std::env::temp_dir().join(format!("apogee-synthetic-runtime-test-{}", Uuid::new_v4()));
        let workspace_root = root.join("workspace");
        let runtime_root = root.join("runtime");
        let state = Arc::new(AppState::new().unwrap());
        let repository = WorkspaceRepository::create(&workspace_root, "First".into()).unwrap();
        let (workspace_id, first_session) =
            state.open_workspace(repository, &runtime_root).unwrap();
        let first_spool_root = runtime_root
            .join("responses")
            .join(first_session.to_string());
        let writer_started = Arc::new(Barrier::new(2));
        let release_writer = Arc::new(Barrier::new(2));
        let worker_state = Arc::clone(&state);
        let worker_started = Arc::clone(&writer_started);
        let worker_release = Arc::clone(&release_writer);
        let worker = thread::spawn(move || {
            let payload: &[u8] = br#"{"synthetic":true}"#;
            worker_state.make_synthetic_with_writer(1, move |spool, id, _| {
                worker_started.wait();
                worker_release.wait();
                Ok(spool.write_stream(id, [payload])?)
            })
        });

        writer_started.wait();
        drop(
            state
                .workspace
                .try_write()
                .expect("Synthetic 写入期间不得持有 workspace 读锁"),
        );
        drop(
            state
                .runtime
                .try_write()
                .expect("Synthetic 写入期间不得持有 spool 读锁"),
        );

        state.close_workspace(workspace_id, first_session).unwrap();
        let reopened = WorkspaceRepository::open(&workspace_root).unwrap();
        let (reopened_workspace, second_session) =
            state.open_workspace(reopened, &runtime_root).unwrap();
        assert_eq!(reopened_workspace, workspace_id);
        assert_ne!(second_session, first_session);
        let second_spool_root = state
            .runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .spool()
            .path_for(Uuid::new_v4())
            .parent()
            .expect("response spool path 必须有父目录")
            .to_path_buf();
        assert!(second_spool_root.ends_with(second_session.to_string()));

        release_writer.wait();
        let stale_error = worker.join().unwrap().unwrap_err();
        assert_eq!(stale_error.to_string(), "WORKSPACE_SESSION_EXPIRED");
        assert!(
            !first_spool_root.exists(),
            "Synthetic runtime lease 释放后必须回收旧 session spool"
        );
        assert!(
            std::fs::read_dir(&second_spool_root)
                .unwrap()
                .next()
                .is_none(),
            "旧 session 的 Synthetic body 不得写入新 session spool"
        );
        assert!(
            state
                .response_scopes
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "旧 session 的 Synthetic 生成不得向新 runtime 发布 scope"
        );
        state
            .close_workspace(reopened_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn startup_synthetic_generation_rejects_runtime_aba_switch() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let root = std::env::temp_dir().join(format!(
            "apogee-startup-synthetic-runtime-test-{}",
            Uuid::new_v4()
        ));
        let state = Arc::new(AppState::new().unwrap());
        let writer_started = Arc::new(Barrier::new(2));
        let release_writer = Arc::new(Barrier::new(2));
        let worker_state = Arc::clone(&state);
        let worker_started = Arc::clone(&writer_started);
        let worker_release = Arc::clone(&release_writer);
        let worker = thread::spawn(move || {
            let payload: &[u8] = br#"{"synthetic":true}"#;
            worker_state.make_synthetic_with_writer(1, move |spool, id, _| {
                worker_started.wait();
                worker_release.wait();
                Ok(spool.write_stream(id, [payload])?)
            })
        });

        writer_started.wait();
        let workspace =
            WorkspaceRepository::create(root.join("workspace"), "First".into()).unwrap();
        let (workspace_id, session_id) = state
            .open_workspace(workspace, root.join("runtime"))
            .unwrap();
        state.close_workspace(workspace_id, session_id).unwrap();

        release_writer.wait();
        let stale_error = worker.join().unwrap().unwrap_err();
        assert_eq!(stale_error.to_string(), "SYNTHETIC_RUNTIME_EXPIRED");
        assert!(
            state
                .response_scopes
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "启动页 ABA 切换后的旧 Synthetic 不得发布 scope"
        );
        let current_spool_root = state
            .runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .spool()
            .path_for(Uuid::new_v4())
            .parent()
            .expect("response spool path 必须有父目录")
            .to_path_buf();
        assert!(
            std::fs::read_dir(current_spool_root)
                .unwrap()
                .next()
                .is_none(),
            "启动页 ABA 切换后的旧 Synthetic body 不得写入当前 spool"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn response_indexing_releases_runtime_locks_and_revalidates_before_publish() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let root = std::env::temp_dir().join(format!(
            "apogee-response-index-session-test-{}",
            Uuid::new_v4()
        ));
        let state = Arc::new(AppState::new().unwrap());
        let first = WorkspaceRepository::create(root.join("first"), "First".into()).unwrap();
        let (first_workspace, first_session) = state
            .open_workspace(first, root.join("runtime-first"))
            .unwrap();
        let first_spool_root = root
            .join("runtime-first")
            .join("responses")
            .join(first_session.to_string());
        let response_id = Uuid::new_v4();
        let body = br#"{"phase":"first"}"#;
        let access = AppState::response_access(first_workspace, first_session);
        state
            .with_workspace_runtime(first_workspace, first_session, |_, runtime| {
                runtime.spool().write_stream(response_id, [&body[..]])?;
                Ok(())
            })
            .unwrap();
        state
            .response_scopes
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(response_id, ResponseScope::Workspace(access));
        state
            .meta
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((
                response_id,
                body.len() as u64,
                Timing::default(),
                Some("application/json".into()),
            ));

        let parser_entered = Arc::new(Barrier::new(2));
        let release_parser = Arc::new(Barrier::new(2));
        let worker_state = Arc::clone(&state);
        let worker_entered = Arc::clone(&parser_entered);
        let worker_release = Arc::clone(&release_parser);
        let worker = thread::spawn(move || {
            worker_state.index_response_for_access_with_parser(
                access,
                response_id,
                move |spool, id| {
                    // `index_response_for_access_with_parser` 进入这里前已经释放第一阶段的
                    // workspace/spool 读锁；用 barrier 让主线程验证这个事实并切换 runtime。
                    worker_entered.wait();
                    worker_release.wait();
                    AppState::parse_response_index(spool, id)
                },
            )
        });

        parser_entered.wait();
        drop(
            state
                .workspace
                .try_write()
                .expect("索引解析期间不得持有 workspace 读锁"),
        );
        drop(
            state
                .runtime
                .try_write()
                .expect("索引解析期间不得持有 spool 读锁"),
        );

        state
            .close_workspace(first_workspace, first_session)
            .unwrap();
        let second = WorkspaceRepository::create(root.join("second"), "Second".into()).unwrap();
        let (second_workspace, second_session) = state
            .open_workspace(second, root.join("runtime-second"))
            .unwrap();

        release_parser.wait();
        let stale_error = worker.join().unwrap().unwrap_err();
        assert_eq!(stale_error.to_string(), "WORKSPACE_SESSION_EXPIRED");
        assert!(
            !first_spool_root.exists(),
            "索引 runtime lease 释放后必须回收旧 session spool"
        );
        assert_eq!(
            state.store.indexed_count(),
            0,
            "旧 session 的锁外解析不得向新 runtime 发布索引"
        );
        state
            .close_workspace(second_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn response_access_and_protocol_never_cross_workspace_sessions() {
        use apogee_core::request::{AuthConfig, Body, RequestSettings, RequestSpec};
        use tauri::http::{header, Request, StatusCode};

        fn request_spec() -> RequestSpec {
            RequestSpec {
                method: "GET".into(),
                url: "https://response-scope.example.test".into(),
                params: Vec::new(),
                path_params: Vec::new(),
                headers: Vec::new(),
                body: Body::None,
                auth: AuthConfig::Inherit,
                settings: RequestSettings::default(),
            }
        }

        fn write_history(
            repository: &WorkspaceRepository,
            source: &std::path::Path,
            response_id: Uuid,
            payload: &[u8],
        ) {
            std::fs::write(source, payload).unwrap();
            repository
                .record_history(
                    1234,
                    PublicRequestPreview::from(&request_spec()),
                    PublicResponseSummary::from(&ResponseSummary::without_http(
                        response_id,
                        payload.len() as u64,
                        Timing::default(),
                    )),
                    source,
                    None,
                    None,
                )
                .unwrap();
        }

        fn protocol_request(uri: String) -> Request<Vec<u8>> {
            Request::builder()
                .uri(uri)
                .header(header::RANGE, "bytes=0-1023")
                .body(Vec::new())
                .unwrap()
        }

        let root =
            std::env::temp_dir().join(format!("apogee-response-session-test-{}", Uuid::new_v4()));
        let response_id = Uuid::new_v4();
        let first_payload = br#"{"workspace":"first"}"#;
        let second_payload = br#"{"workspace":"second"}"#;
        let first_root = root.join("first");
        let second_root = root.join("second");
        let first = WorkspaceRepository::create(&first_root, "First".into()).unwrap();
        write_history(
            &first,
            &root.join("first-source.body"),
            response_id,
            first_payload,
        );

        let state = AppState::new().unwrap();
        let (first_workspace, first_session) = state
            .open_workspace(first, root.join("runtime-first"))
            .unwrap();
        state
            .read_history(first_workspace, first_session, response_id)
            .unwrap();
        assert_eq!(
            state
                .response_range(first_workspace, first_session, response_id, 0, 1024)
                .unwrap(),
            first_payload
        );
        assert!(
            state
                .index_response(first_workspace, first_session, response_id)
                .unwrap()
                .indexed
        );
        assert_eq!(state.store.indexed_count(), 1);
        let first_protocol = crate::protocol::handle(
            &state,
            &protocol_request(format!(
                "/response/{response_id}?workspaceId={first_workspace}&sessionId={first_session}"
            )),
        );
        assert_eq!(first_protocol.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(first_protocol.body().as_slice(), first_payload);

        state
            .close_workspace(first_workspace, first_session)
            .unwrap();
        assert_eq!(state.store.indexed_count(), 0);

        let second = WorkspaceRepository::create(&second_root, "Second".into()).unwrap();
        write_history(
            &second,
            &root.join("second-source.body"),
            response_id,
            second_payload,
        );
        let (second_workspace, second_session) = state
            .open_workspace(second, root.join("runtime-second"))
            .unwrap();
        state
            .read_history(second_workspace, second_session, response_id)
            .unwrap();

        let stale_read = state
            .response_range(first_workspace, first_session, response_id, 0, 1024)
            .unwrap_err();
        assert_eq!(stale_read.to_string(), "WORKSPACE_SESSION_EXPIRED");
        let stale_index = state
            .index_response(first_workspace, first_session, response_id)
            .unwrap_err();
        assert_eq!(stale_index.to_string(), "WORKSPACE_SESSION_EXPIRED");
        let stale_protocol = crate::protocol::handle(
            &state,
            &protocol_request(format!(
                "/response/{response_id}?workspaceId={first_workspace}&sessionId={first_session}"
            )),
        );
        assert_eq!(stale_protocol.status(), StatusCode::NOT_FOUND);
        let bare_protocol = crate::protocol::handle(
            &state,
            &protocol_request(format!("/response/{response_id}")),
        );
        assert_eq!(bare_protocol.status(), StatusCode::FORBIDDEN);

        let second_protocol = crate::protocol::handle(
            &state,
            &protocol_request(format!(
                "/response/{response_id}?workspaceId={second_workspace}&sessionId={second_session}"
            )),
        );
        assert_eq!(second_protocol.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(second_protocol.body().as_slice(), second_payload);
        assert!(
            state
                .index_response(second_workspace, second_session, response_id)
                .unwrap()
                .indexed
        );
        assert_eq!(
            state
                .response_tree_page(second_workspace, second_session, response_id, "", 0, 10)
                .unwrap()
                .len(),
            1
        );
        state
            .close_workspace(second_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_write_requires_response_owned_by_active_session() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let root = std::env::temp_dir().join(format!(
            "apogee-history-write-scope-test-{}",
            Uuid::new_v4()
        ));
        let workspace_root = root.join("workspace");
        let runtime_root = root.join("runtime");
        let state = AppState::new().unwrap();
        let first = WorkspaceRepository::create(&workspace_root, "First".into()).unwrap();
        let (workspace_id, first_session) = state.open_workspace(first, &runtime_root).unwrap();
        let response_id = Uuid::new_v4();
        let body = br#"{"leftover":true}"#;
        state
            .with_workspace_runtime(workspace_id, first_session, |_, runtime| {
                runtime.spool().write_stream(response_id, [&body[..]])?;
                Ok(())
            })
            .unwrap();
        state
            .response_scopes
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                response_id,
                ResponseScope::Workspace(AppState::response_access(workspace_id, first_session)),
            );
        let stale_body = runtime_root
            .join("responses")
            .join(first_session.to_string())
            .join(format!("{response_id}.bin"));
        assert!(stale_body.exists());

        state.close_workspace(workspace_id, first_session).unwrap();
        let reopened = WorkspaceRepository::open(&workspace_root).unwrap();
        let (reopened_workspace, reopened_session) =
            state.open_workspace(reopened, &runtime_root).unwrap();
        assert_eq!(reopened_workspace, workspace_id);
        assert!(
            !stale_body.exists(),
            "无外部 runtime lease 的关闭 session 必须回收遗留 response body"
        );

        let operation_entered = AtomicBool::new(false);
        let error = state
            .with_workspace_and_response(
                reopened_workspace,
                reopened_session,
                response_id,
                |_, _| {
                    operation_entered.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "RESPONSE_NOT_FOUND");
        assert!(
            !operation_entered.load(Ordering::SeqCst),
            "无 owner 的遗留 body 不得进入历史持久化操作"
        );
        state
            .close_workspace(reopened_workspace, reopened_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn synthetic_response_uses_explicit_scope_while_workspace_is_open() {
        let root =
            std::env::temp_dir().join(format!("apogee-synthetic-scope-test-{}", Uuid::new_v4()));
        let state = AppState::new().unwrap();
        let workspace =
            WorkspaceRepository::create(root.join("workspace"), "First".into()).unwrap();
        let (workspace_id, session_id) = state
            .open_workspace(workspace, root.join("runtime"))
            .unwrap();

        let synthetic = state.make_synthetic(1).unwrap();
        assert!(state
            .synthetic_response_range(synthetic.id, 0, 64)
            .unwrap()
            .starts_with(b"{"));
        assert!(
            state
                .index_synthetic_response(synthetic.id)
                .unwrap()
                .indexed,
            "打开工作区时 Spike 的显式 Synthetic scope 仍应可索引"
        );
        let normal_access = state
            .response_range(workspace_id, session_id, synthetic.id, 0, 64)
            .unwrap_err();
        assert_eq!(normal_access.to_string(), "RESPONSE_SESSION_EXPIRED");

        state.close_workspace(workspace_id, session_id).unwrap();
        let stale_synthetic = state
            .synthetic_response_range(synthetic.id, 0, 64)
            .unwrap_err();
        assert_eq!(stale_synthetic.to_string(), "RESPONSE_NOT_FOUND");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_response_access_cannot_run_terminal_callback_in_new_session() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let root = std::env::temp_dir().join(format!(
            "apogee-terminal-event-session-test-{}",
            Uuid::new_v4()
        ));
        let state = AppState::new().unwrap();
        let first = WorkspaceRepository::create(root.join("first"), "First".into()).unwrap();
        let (first_workspace, first_session) = state
            .open_workspace(first, root.join("runtime-first"))
            .unwrap();
        let lease = state
            .begin_request(first_workspace, first_session, Uuid::new_v4(), None)
            .unwrap();
        let (_, _cancellation, access) = lease.into_execution_parts();

        state
            .close_workspace(first_workspace, first_session)
            .unwrap();
        let second = WorkspaceRepository::create(root.join("second"), "Second".into()).unwrap();
        let (second_workspace, second_session) = state
            .open_workspace(second, root.join("runtime-second"))
            .unwrap();

        let callback_entered = AtomicBool::new(false);
        let error = state
            .with_active_response_access(access, || {
                callback_entered.store(true, Ordering::SeqCst);
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "WORKSPACE_SESSION_EXPIRED");
        assert!(
            !callback_entered.load(Ordering::SeqCst),
            "失效 lease 不得投递终结事件回调"
        );
        state
            .close_workspace(second_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn request_lease_cannot_register_late_response_in_new_session() {
        use apogee_core::model::{ActualRequest, ResponseMeta};

        let root = std::env::temp_dir().join(format!(
            "apogee-late-response-session-test-{}",
            Uuid::new_v4()
        ));
        let state = AppState::new().unwrap();
        let first = WorkspaceRepository::create(root.join("first"), "First".into()).unwrap();
        let (first_workspace, first_session) = state
            .open_workspace(first, root.join("runtime-first"))
            .unwrap();
        let response_id = Uuid::new_v4();
        let lease = state
            .begin_request(first_workspace, first_session, response_id, None)
            .unwrap();
        let (_, _cancellation, access) = lease.into_execution_parts();

        state
            .close_workspace(first_workspace, first_session)
            .unwrap();
        let second = WorkspaceRepository::create(root.join("second"), "Second".into()).unwrap();
        let (second_workspace, second_session) = state
            .open_workspace(second, root.join("runtime-second"))
            .unwrap();

        let error = state
            .record_http_response(
                access,
                &ResponseMeta {
                    id: response_id,
                    status: 200,
                    status_text: "OK".into(),
                    http_version: "HTTP/1.1".into(),
                    headers: Vec::new(),
                    size_bytes: 2,
                    body_path: root.join("late-response.bin"),
                    content_type: Some("application/json".into()),
                    charset: Some("utf-8".into()),
                    timing: Timing::default(),
                    actual_request: ActualRequest {
                        method: "GET".into(),
                        url: "https://late.example.test".into(),
                        path_params: Vec::new(),
                        headers: Vec::new(),
                        common_header_names: Vec::new(),
                        body: None,
                        body_params: Vec::new(),
                        sensitive_path_param_names: Vec::new(),
                        sensitive_path_values: Vec::new(),
                        sensitive_query_names: Vec::new(),
                        sensitive_header_names: Vec::new(),
                    },
                    from_cache: false,
                    truncated: false,
                },
            )
            .unwrap_err();
        assert_eq!(error.to_string(), "WORKSPACE_SESSION_EXPIRED");
        assert!(
            state
                .meta
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "失效 lease 不得向新 session 写入响应元数据"
        );
        state
            .close_workspace(second_workspace, second_session)
            .unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }
}
