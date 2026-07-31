//! 应用状态：响应体 spool + JSON SpanIndex 缓存。

use anyhow::{Context, Result};
use memmap2::MmapOptions;
use std::fs::File;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::Instant;
use uuid::Uuid;
use apogee_core::model::{RenderTier, Timing};
use apogee_core::response::{
    scalar_preview, synth_json_payload, ResponseStore, ResponseSummary, SpanIndex, Spool, TreeNode,
    PREVIEW_READ_BYTES,
};

pub struct AppState {
    pub spool: Spool,
    pub store: ResponseStore,
    /// 只存元信息（体积、耗时），**不存 body**。
    meta: RwLock<Vec<(Uuid, u64, Timing, Option<String>)>>,
}

impl AppState {
    pub fn new() -> Result<Self> {
        // 响应体缓存目录。真实产品里放应用数据目录，
        // 并按 LRU 限额淘汰（默认 500MB，见 03 文档 §4.2）。
        let root = std::env::temp_dir().join("apogee-spool");
        Ok(Self {
            spool: Spool::new(root)?,
            store: ResponseStore::new(),
            meta: RwLock::new(Vec::new()),
        })
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

        let size = self.spool.write_stream(id, chunks)?;
        let gen_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let timing = Timing {
            total_ms: gen_ms,
            download_ms: gen_ms,
            ..Default::default()
        };
        self.meta
            .write()
            .unwrap()
            .push((id, size, timing, Some("application/json".into())));

        Ok(self.build_summary(id, size, timing, Some("application/json".into())))
    }

    pub fn summary(&self, id: Uuid) -> Result<ResponseSummary> {
        let m = self.meta.read().unwrap();
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
        let tier = RenderTier::from_size(size);
        let index = self.store.index(id);
        ResponseSummary {
            id,
            size_bytes: size,
            tier,
            syntax_highlight: tier.syntax_highlight(),
            content_type,
            indexed: index.is_some(),
            total_nodes: index.as_ref().map(|item| item.total_nodes()),
            index_bytes: index.as_ref().map(|item| item.estimated_bytes()),
            timing,
        }
    }

    /// 直接对不可变 spool 文件做只读 mmap 并构建 SpanIndex。
    ///
    /// mmap 只建立虚拟地址映射，不把完整 body 复制到 Rust 堆；索引构建结束后立即解除映射，
    /// 常驻内存只剩固定 32 字节/节点的元数据和去重后的对象键。
    pub fn index_response(&self, id: Uuid) -> Result<ResponseSummary> {
        let path = self.spool.path_for(id);
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
                    let bytes = self.spool.read_range(id, span.start, requested)?;
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
