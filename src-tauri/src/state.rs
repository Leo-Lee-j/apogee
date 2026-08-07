//! 应用状态：响应体 spool + JSON SpanIndex 缓存 + 请求中注册表。

use crate::workspace::WorkspaceRepository;
use anyhow::{bail, Context, Result};
use apogee_core::model::{ResponseMeta, Timing};
use apogee_core::response::{
    scalar_preview, synth_json_payload, ResponseStore, ResponseSummary, SpanIndex, Spool, TreeNode,
    PREVIEW_READ_BYTES,
};
use memmap2::MmapOptions;
use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::Instant;
use tokio::sync::oneshot;
use uuid::Uuid;

pub struct WorkspaceSession {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub repository: WorkspaceRepository,
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

pub struct AppState {
    pub spool: RwLock<Spool>,
    pub store: ResponseStore,
    /// 只存元信息（体积、耗时），**不存 body**。
    pub(crate) meta: RwLock<Vec<(Uuid, u64, Timing, Option<String>)>>,
    semantic_meta: RwLock<HashMap<Uuid, ResponseSummary>>,
    /// 当前窗口绑定的工作区会话。启动入口阶段为 None。
    pub workspace: RwLock<Option<WorkspaceSession>>,
    /// 请求中注册表。key 是 ExecHandle.id（= 响应 id）。
    in_flight: RwLock<HashMap<Uuid, InFlight>>,
}

impl AppState {
    pub fn new() -> Result<Self> {
        let root = std::env::temp_dir().join("apogee-spool");
        Ok(Self {
            spool: RwLock::new(Spool::new(root)?),
            store: ResponseStore::new(),
            meta: RwLock::new(Vec::new()),
            semantic_meta: RwLock::new(HashMap::new()),
            workspace: RwLock::new(None),
            in_flight: RwLock::new(HashMap::new()),
        })
    }

    pub fn open_workspace(
        &self,
        repository: WorkspaceRepository,
        runtime_root: impl AsRef<std::path::Path>,
    ) -> Result<(Uuid, Uuid)> {
        let workspace_id = repository.id()?;
        let session_id = Uuid::new_v4();
        let mut guard = self
            .workspace
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // 正常流程前端会先 close 再 open；这里仍残留会话，说明是前端重载或中途
        // 失败遗留的孤儿会话（前端已回到启动页且丢失 sessionId，无法再 close）。
        // 直接替换而不是报 WORKSPACE_ALREADY_OPEN，否则用户会卡死在启动页，
        // 无论点哪个工作区都进不去。
        let replaced = guard.is_some();
        let spool = Spool::new(runtime_root.as_ref().join("responses"))?;
        *self.spool.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = spool;
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

    pub fn close_workspace(&self, workspace_id: Uuid, session_id: Uuid) -> Result<()> {
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
        *guard = None;
        drop(guard);
        self.cancel_all_inflight();
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

    /// 注册一个正在执行的请求，返回 receiver 给 execute() 当取消信号。
    pub fn register_inflight(&self, id: Uuid, tab_id: Option<String>) -> oneshot::Receiver<()> {
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

    /// 记录完成的 HTTP 响应元信息，供后续 query 拿到 HTTP 语义。
    pub fn record_http_meta(
        &self,
        id: Uuid,
        size: u64,
        timing: Timing,
        content_type: Option<String>,
    ) -> Result<()> {
        self.meta
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((id, size, timing, content_type));
        Ok(())
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
        let id = Uuid::new_v4();
        let t0 = Instant::now();

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
            s[start..end].as_bytes().to_vec()
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

        let spool = self.spool.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let size = spool.write_stream(id, chunks)?;
        let gen_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let timing = Timing {
            total_ms: gen_ms,
            download_ms: gen_ms,
            ..Default::default()
        };
        self.meta
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((id, size, timing, Some("application/json".into())));

        Ok(self.build_summary(id, size, timing, Some("application/json".into())))
    }

    pub fn summary(&self, id: Uuid) -> Result<ResponseSummary> {
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
    pub fn summarize_http(&self, meta: &ResponseMeta) -> Result<ResponseSummary> {
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

    pub fn restore_history_response(
        &self,
        mut summary: ResponseSummary,
        body: &std::path::Path,
    ) -> Result<ResponseSummary> {
        let spool = self.spool.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let target = spool.path_for(summary.id);
        if !target.exists() {
            std::fs::copy(body, &target)
                .with_context(|| format!("恢复历史响应体失败：{}", body.display()))?;
        }
        summary.indexed = false;
        summary.total_nodes = None;
        summary.index_bytes = None;
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
    }

    /// 直接对不可变 spool 文件做只读 mmap 并构建 SpanIndex。
    ///
    /// mmap 只建立虚拟地址映射，不把完整 body 复制到 Rust 堆；索引构建结束后立即解除映射，
    /// 常驻内存只剩固定 32 字节/节点的元数据和去重后的对象键。
    pub fn index_response(&self, id: Uuid) -> Result<ResponseSummary> {
        let spool = self.spool.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let path = spool.path_for(id);
        let file = File::open(&path)
            .with_context(|| format!("打开响应 spool 失败：{}", path.display()))?;
        // SAFETY: spool 文件写完后通过原子 rename 发布，发布后只读且不会原地改写；
        // 映射的生命周期不超过 file 和本函数，SpanIndex 返回时不借用映射内容。
        let mapped = unsafe { MmapOptions::new().map(&file) }
            .with_context(|| format!("映射响应 spool 失败：{}", path.display()))?;
        let index = SpanIndex::parse(&mapped).map_err(|error| anyhow::anyhow!(error))?;
        drop(mapped);
        self.store.insert_index(id, index);
        self.summary(id)
    }

    /// 读取一页节点，并只为这一页的标量按 span 拉取最多 4KB 预览。
    pub fn tree_page(
        &self,
        id: Uuid,
        path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TreeNode>> {
        let spool = self.spool.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
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
            .spool
            .read()
            .unwrap()
            .path_for(Uuid::new_v4())
            .starts_with(root.join("runtime-first").join("responses")));
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
            .spool
            .read()
            .unwrap()
            .path_for(Uuid::new_v4())
            .starts_with(root.join("runtime-second").join("responses")));

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
        state.close_workspace(second_workspace, second_session).unwrap();

        std::fs::remove_dir_all(root).unwrap();
    }
}
