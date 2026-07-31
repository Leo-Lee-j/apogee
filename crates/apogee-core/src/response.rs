//! 响应体存储与按需读取 —— 这是 Spike S2 的 Rust 侧实现。
//!
//! # 为什么这么设计（03 文档 §4.3）
//!
//! Electron 系客户端处理大响应体的路径是：整体读进内存 → 主线程 `JSON.parse`
//! → DOM 全量渲染。10MB 就开始掉帧，100MB 直接崩。
//!
//! Apogee 的路径：
//! 1. 网络流边收边写盘，内存只留元信息
//! 2. JSON 在 Rust 侧扫描一次，索引只保留紧凑节点元数据与字节区间
//! 3. 前端每次只取视口内的一页节点，标量预览再从 spool 按区间读取
//!
//! **前端和 Rust 都不常驻完整响应字符串或 `serde_json::Value`。**

use crate::model::{RenderTier, Timing};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// 单次 Range 读取的上限。前端一次最多拉这么多，防止误用把大 body 整体拉过去。
pub const MAX_RANGE_BYTES: u64 = 4 * 1024 * 1024;

/// 树节点分页的默认页大小。视口约 30 行，加缓冲取 200 足够。
pub const DEFAULT_TREE_PAGE: usize = 200;

/// 标量预览最多读取的原始字节数。即使一个 JSON 字符串有几百 MB，也只读这一小段。
pub const PREVIEW_READ_BYTES: u64 = 4 * 1024;

/// 默认 JSON 索引内存预算。超出后按最近最少使用淘汰旧索引；最新索引始终保留。
pub const DEFAULT_INDEX_CACHE_BYTES: usize = 128 * 1024 * 1024;

const MAX_JSON_DEPTH: usize = 256;
const NO_NODE: u32 = u32::MAX;
const META_KIND_MASK: u32 = 0b111;
const META_INDEX_KEY: u32 = 1 << 3;
const META_CHILD_SHIFT: u32 = 4;
const MAX_CHILDREN: usize = (u32::MAX >> META_CHILD_SHIFT) as usize;

// ─────────────────────────────────────────────────────────────────────
// 磁盘 spool
// ─────────────────────────────────────────────────────────────────────

/// 响应体磁盘缓存。
///
/// 每个响应体一个文件，前端通过 `apogee://response/{id}` + Range 头读取。
///
/// `Clone` 是为了让 spawned task 能拿到自己的引用，
/// 里面的 `PathBuf` clone 很便宜。
#[derive(Clone)]
pub struct Spool {
    root: PathBuf,
}

impl Spool {
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn path_for(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.bin"))
    }

    /// 流式写入：分块落盘，全程不在内存里持有完整 body。
    ///
    /// 真实场景下 `chunks` 来自 reqwest 的响应流；Spike 里来自生成器。
    pub fn write_stream<I, B>(&self, id: Uuid, chunks: I) -> std::io::Result<u64>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let path = self.path_for(id);
        // 原子写：先写临时文件再 rename，崩溃不留半截文件（PRD §13 R9）
        let tmp = path.with_extension("part");
        let mut written = 0u64;
        {
            let mut f = File::create(&tmp)?;
            for chunk in chunks {
                let bytes = chunk.as_ref();
                f.write_all(bytes)?;
                written += bytes.len() as u64;
            }
            f.flush()?;
        }
        fs::rename(&tmp, &path)?;
        Ok(written)
    }

    /// 增量写入句柄，供 async 场景逐块落盘。
    ///
    /// 为什么不能用 `write_stream`：它的签名是 `IntoIterator`，而 reqwest 的响应流
    /// 是 `chunk().await` —— 一个同步迭代器无法表达「下一块要 await」。硬要适配就得
    /// 先把整个 body 收进 `Vec` 再交给迭代器，那正好破坏了流式落盘的全部意义。
    ///
    /// 原子性与 `write_stream` 一致：先写 `.part`，`finish()` 时才 rename。
    pub fn writer(&self, id: Uuid) -> std::io::Result<SpoolWriter> {
        let final_path = self.path_for(id);
        let temp_path = final_path.with_extension("part");
        Ok(SpoolWriter {
            file: Some(File::create(&temp_path)?),
            temp_path,
            final_path,
            written: 0,
        })
    }

    /// 按 Range 读取分片。这是自定义协议 handler 的底层实现。
    ///
    /// 只 seek + 读需要的那一段，不整体加载 —— 100MB 文件读首屏 64KB 的成本
    /// 与 64KB 文件相同。
    pub fn read_range(&self, id: Uuid, start: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let len = len.min(MAX_RANGE_BYTES);
        let path = self.path_for(id);
        let mut f = File::open(&path)?;
        let total = f.metadata()?.len();
        if start >= total {
            return Ok(Vec::new());
        }
        let take = len.min(total - start);
        f.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0u8; take as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn size_of(&self, id: Uuid) -> std::io::Result<u64> {
        Ok(File::open(self.path_for(id))?.metadata()?.len())
    }

    pub fn remove(&self, id: Uuid) -> std::io::Result<()> {
        let path = self.path_for(id);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// 增量写入句柄。由 `Spool::writer` 创建，见那里的说明。
pub struct SpoolWriter {
    /// `Option` 是为了能在 rename **之前**显式关掉句柄。
    ///
    /// Windows 上重命名一个仍被打开的文件会失败（sharing violation）。
    /// 这类问题在 macOS 上开发时完全不会出现，到 Windows 才炸 —— 而 Windows
    /// 是我们的首要目标平台（决策 D2）。
    file: Option<File>,
    temp_path: PathBuf,
    final_path: PathBuf,
    written: u64,
}

impl SpoolWriter {
    /// 追加一块。调用方负责在块之间检查取消与上报进度。
    pub fn write(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.write_all(chunk)?;
            self.written += chunk.len() as u64;
        }
        Ok(())
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    /// 收尾：flush、关句柄、原子发布。返回总字节数。
    pub fn finish(mut self) -> std::io::Result<u64> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
            // drop 掉句柄再 rename，理由见 `file` 字段的注释
            drop(file);
        }
        fs::rename(&self.temp_path, &self.final_path)?;
        Ok(self.written)
    }
}

impl Drop for SpoolWriter {
    /// 没走到 `finish()` 就被丢弃（请求取消或出错）时清掉临时文件。
    ///
    /// 不清理的后果是 `.part` 文件在 spool 目录里永久堆积：LRU 淘汰只认正式
    /// 文件名，扫不到这些残片。
    fn drop(&mut self) {
        if self.file.is_some() {
            let _ = fs::remove_file(&self.temp_path);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// JSON SpanIndex
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    Object = 0,
    Array = 1,
    String = 2,
    Number = 3,
    Bool = 4,
    Null = 5,
}

impl NodeKind {
    fn from_meta(meta: u32) -> Self {
        match meta & META_KIND_MASK {
            0 => Self::Object,
            1 => Self::Array,
            2 => Self::String,
            3 => Self::Number,
            4 => Self::Bool,
            5 => Self::Null,
            // 防御式:文件被外部改动 / 版本不兼容时,落到 Null 而不是 abort 进程。
            // 视图层拿到的是"未知节点显示为空",而不是整个 app 闪退。
            _ => {
                tracing::warn!("解析到未知节点类型 meta={:#x},按 Null 处理", meta);
                Self::Null
            }
        }
    }

    fn is_container(self) -> bool {
        matches!(self, Self::Object | Self::Array)
    }
}

/// spool 文件内的半开字节区间 `[start, end)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteSpan {
    pub start: u64,
    pub end: u64,
}

impl ByteSpan {
    pub fn len(self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

/// 一个树节点。**注意没有 `children` 字段** —— 前端拿到的是扁平数据，
/// 不做递归构造（03 文档 §4.4）。需要子节点时按 path 再取一页。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    /// JSON Pointer 风格路径，如 `/data/list/0/name`
    pub path: String,
    /// 在父节点中的键名（对象）或下标（数组）
    pub key: String,
    pub kind: NodeKind,
    /// 标量的值预览；容器为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// 容器的子节点数量；标量为 0
    pub child_count: usize,
    pub depth: usize,
}

/// 索引返回的节点元数据。标量的 `value_span` 由调用方从 spool 按需读取。
#[derive(Debug, Clone)]
pub struct IndexedTreeNode {
    pub node: TreeNode,
    pub value_span: Option<ByteSpan>,
}

/// 每个节点固定 32 字节：不保存值、不保存完整 path，也不为每个节点分配子 Vec。
///
/// `meta` 的低 3 位是 kind，第 3 位表示 key 是数组下标，高 28 位是 child_count。
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct SpanNode {
    value_start: u64,
    value_end: u64,
    parent: u32,
    first_child: u32,
    key: u32,
    meta: u32,
}

const _: () = assert!(size_of::<SpanNode>() == 32);

impl SpanNode {
    fn new(span: ByteSpan, parent: u32, key: u32, index_key: bool, kind: NodeKind) -> Self {
        Self {
            value_start: span.start,
            value_end: span.end,
            parent,
            first_child: NO_NODE,
            key,
            meta: kind as u32 | if index_key { META_INDEX_KEY } else { 0 },
        }
    }

    fn kind(self) -> NodeKind {
        NodeKind::from_meta(self.meta)
    }

    fn span(self) -> ByteSpan {
        ByteSpan {
            start: self.value_start,
            end: self.value_end,
        }
    }

    fn child_count(self) -> usize {
        (self.meta >> META_CHILD_SHIFT) as usize
    }

    fn set_children(&mut self, first: u32, count: usize) {
        debug_assert!(count <= MAX_CHILDREN);
        self.first_child = if count == 0 { NO_NODE } else { first };
        self.meta = (self.meta & 0xF) | ((count as u32) << META_CHILD_SHIFT);
    }

    fn has_index_key(self) -> bool {
        self.meta & META_INDEX_KEY != 0
    }
}

/// JSON 语法或索引规模错误。错误位置始终是 spool 的字节偏移。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanIndexError {
    pub offset: usize,
    pub message: String,
}

impl SpanIndexError {
    fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }
}

impl fmt::Display for SpanIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}（字节 {}）", self.message, self.offset)
    }
}

impl std::error::Error for SpanIndexError {}

/// 紧凑 JSON 字节区间索引。
///
/// 索引不拥有 body，也不包含 `serde_json::Value`。对象键只在全局字符串池中保留一份，
/// 数组下标直接编码为 u32；节点 path 在分页返回时按当前父路径即时生成。
pub struct SpanIndex {
    nodes: Vec<SpanNode>,
    keys: Vec<String>,
}

impl SpanIndex {
    /// 从 JSON 字节构建索引。调用方可传普通 slice 或只读 mmap；返回后不借用输入。
    pub fn parse(bytes: &[u8]) -> Result<Self, SpanIndexError> {
        let parser = JsonScanner::new(bytes);
        let (root, total_nodes) = parser.document()?;
        if total_nodes > u32::MAX as usize {
            return Err(SpanIndexError::new(0, "JSON 节点数超过索引上限"));
        }

        let mut builder = SpanIndexBuilder {
            parser,
            nodes: Vec::with_capacity(total_nodes),
            keys: Vec::new(),
            key_ids: HashMap::new(),
        };
        builder
            .nodes
            .push(SpanNode::new(root.span(), NO_NODE, 0, false, root.kind));
        builder.index_children(0, 0)?;

        if builder.nodes.len() != total_nodes {
            return Err(SpanIndexError::new(0, "JSON 索引节点计数不一致"));
        }
        builder.keys.shrink_to_fit();
        Ok(Self {
            nodes: builder.nodes,
            keys: builder.keys,
        })
    }

    pub fn total_nodes(&self) -> usize {
        self.nodes.len()
    }

    /// 索引实际持有的近似堆内存，用于预算和 Spike；不含 Arc/HashMap 缓存壳的少量开销。
    pub fn estimated_bytes(&self) -> usize {
        self.nodes.capacity() * size_of::<SpanNode>()
            + self.keys.capacity() * size_of::<String>()
            + self.keys.iter().map(String::capacity).sum::<usize>()
    }

    /// 取指定路径下的一页子节点。标量值只返回 span，不在这里触碰 body 文件。
    pub fn children(&self, path: &str, offset: usize, limit: usize) -> Vec<IndexedTreeNode> {
        let Some(parent_id) = self.resolve_id(path) else {
            return Vec::new();
        };
        let parent = self.nodes[parent_id as usize];
        let count = parent.child_count();
        if count == 0 || offset >= count {
            return Vec::new();
        }

        let take = if limit == 0 { DEFAULT_TREE_PAGE } else { limit }.min(count - offset);
        let first = parent.first_child as usize + offset;
        let depth = pointer_depth(path) + 1;
        (first..first + take)
            .map(|id| {
                let raw = self.nodes[id];
                let key = if raw.has_index_key() {
                    raw.key.to_string()
                } else {
                    self.keys[raw.key as usize].clone()
                };
                let escaped = escape_pointer_segment(&key);
                let node_path = format!("{path}/{escaped}");
                let kind = raw.kind();
                IndexedTreeNode {
                    node: TreeNode {
                        path: node_path,
                        key,
                        kind,
                        preview: None,
                        child_count: raw.child_count(),
                        depth,
                    },
                    value_span: (!kind.is_container()).then(|| raw.span()),
                }
            })
            .collect()
    }

    /// 查询路径对应的原文区间。用于后续 JSONPath/复制节点，不需要加载完整 body。
    pub fn span(&self, pointer: &str) -> Option<ByteSpan> {
        self.resolve_id(pointer)
            .map(|id| self.nodes[id as usize].span())
    }

    fn resolve_id(&self, pointer: &str) -> Option<u32> {
        if pointer.is_empty() {
            return (!self.nodes.is_empty()).then_some(0);
        }
        let rest = pointer.strip_prefix('/')?;
        let mut current = 0u32;

        for encoded in rest.split('/') {
            let segment = decode_pointer_segment(encoded)?;
            let node = self.nodes[current as usize];
            let count = node.child_count();
            if count == 0 {
                return None;
            }

            current = match node.kind() {
                NodeKind::Array => {
                    let index = segment.parse::<usize>().ok()?;
                    if index >= count {
                        return None;
                    }
                    node.first_child.checked_add(index as u32)?
                }
                NodeKind::Object => {
                    let first = node.first_child as usize;
                    (first..first + count).find(|&id| {
                        let child = self.nodes[id];
                        !child.has_index_key() && self.keys[child.key as usize] == segment
                    })? as u32
                }
                _ => return None,
            };
        }
        Some(current)
    }
}

struct SpanIndexBuilder<'a> {
    parser: JsonScanner<'a>,
    nodes: Vec<SpanNode>,
    keys: Vec<String>,
    key_ids: HashMap<String, u32>,
}

impl SpanIndexBuilder<'_> {
    fn index_children(&mut self, node_id: u32, depth: usize) -> Result<(), SpanIndexError> {
        if depth > MAX_JSON_DEPTH {
            let offset = self.nodes[node_id as usize].value_start as usize;
            return Err(SpanIndexError::new(offset, "JSON 嵌套层级超过上限"));
        }

        let parent = self.nodes[node_id as usize];
        if !parent.kind().is_container() {
            return Ok(());
        }

        let descriptors = self.parser.direct_children(parent)?;
        if descriptors.len() > MAX_CHILDREN {
            return Err(SpanIndexError::new(
                parent.value_start as usize,
                "单个容器的子节点数超过索引上限",
            ));
        }
        let first = self.nodes.len() as u32;
        self.nodes[node_id as usize].set_children(first, descriptors.len());

        for descriptor in &descriptors {
            let (key, index_key) = match descriptor.key {
                ChildKey::Array(index) => (index, true),
                ChildKey::Object(span) => {
                    let raw = &self.parser.bytes[span.start as usize..span.end as usize];
                    let decoded: String = serde_json::from_slice(raw).map_err(|error| {
                        SpanIndexError::new(span.start as usize, format!("对象键解析失败：{error}"))
                    })?;
                    (self.intern_key(decoded), false)
                }
            };
            self.nodes.push(SpanNode::new(
                descriptor.value.span(),
                node_id,
                key,
                index_key,
                descriptor.value.kind,
            ));
        }

        let count = descriptors.len();
        drop(descriptors);
        for child_id in first..first + count as u32 {
            if self.nodes[child_id as usize].kind().is_container() {
                self.index_children(child_id, depth + 1)?;
            }
        }
        Ok(())
    }

    fn intern_key(&mut self, key: String) -> u32 {
        if let Some(&id) = self.key_ids.get(&key) {
            return id;
        }
        let id = self.keys.len() as u32;
        self.keys.push(key.clone());
        self.key_ids.insert(key, id);
        id
    }
}

#[derive(Debug, Clone, Copy)]
struct ScannedValue {
    start: usize,
    end: usize,
    kind: NodeKind,
}

impl ScannedValue {
    fn span(self) -> ByteSpan {
        ByteSpan {
            start: self.start as u64,
            end: self.end as u64,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ChildKey {
    Object(ByteSpan),
    Array(u32),
}

#[derive(Debug, Clone, Copy)]
struct ChildDescriptor {
    key: ChildKey,
    value: ScannedValue,
}

/// 无分配 JSON 扫描器。首次扫描负责完整语法校验与节点计数；构建阶段只为每个
/// 容器暂存一层直接子节点描述，随后立即释放，不构造递归 Value 树。
#[derive(Clone, Copy)]
struct JsonScanner<'a> {
    bytes: &'a [u8],
}

impl<'a> JsonScanner<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn document(self) -> Result<(ScannedValue, usize), SpanIndexError> {
        let start = self.skip_ws(0);
        if start == self.bytes.len() {
            return Err(SpanIndexError::new(start, "JSON 为空"));
        }
        let mut count = 0usize;
        let root = self.scan_value(start, 0, &mut count)?;
        let trailing = self.skip_ws(root.end);
        if trailing != self.bytes.len() {
            return Err(SpanIndexError::new(trailing, "JSON 根值后存在多余内容"));
        }
        Ok((root, count))
    }

    fn direct_children(self, parent: SpanNode) -> Result<Vec<ChildDescriptor>, SpanIndexError> {
        match parent.kind() {
            NodeKind::Object => self.object_children(parent.value_start as usize),
            NodeKind::Array => self.array_children(parent.value_start as usize),
            _ => Ok(Vec::new()),
        }
    }

    fn object_children(self, start: usize) -> Result<Vec<ChildDescriptor>, SpanIndexError> {
        let mut out = Vec::new();
        let mut pos = self.skip_ws(start + 1);
        if self.byte(pos) == Some(b'}') {
            return Ok(out);
        }

        loop {
            let key_start = self.skip_ws(pos);
            let key_end = self.scan_string(key_start)?;
            pos = self.skip_ws(key_end);
            if self.byte(pos) != Some(b':') {
                return Err(SpanIndexError::new(pos, "对象键后缺少冒号"));
            }
            pos = self.skip_ws(pos + 1);
            let mut ignored = 0usize;
            let value = self.scan_value(pos, 1, &mut ignored)?;
            out.push(ChildDescriptor {
                key: ChildKey::Object(ByteSpan {
                    start: key_start as u64,
                    end: key_end as u64,
                }),
                value,
            });
            pos = self.skip_ws(value.end);
            match self.byte(pos) {
                Some(b',') => pos = self.skip_ws(pos + 1),
                Some(b'}') => return Ok(out),
                _ => return Err(SpanIndexError::new(pos, "对象成员后应为逗号或右花括号")),
            }
        }
    }

    fn array_children(self, start: usize) -> Result<Vec<ChildDescriptor>, SpanIndexError> {
        let mut out = Vec::new();
        let mut pos = self.skip_ws(start + 1);
        if self.byte(pos) == Some(b']') {
            return Ok(out);
        }

        let mut index = 0u32;
        loop {
            let mut ignored = 0usize;
            let value = self.scan_value(pos, 1, &mut ignored)?;
            out.push(ChildDescriptor {
                key: ChildKey::Array(index),
                value,
            });
            index = index
                .checked_add(1)
                .ok_or_else(|| SpanIndexError::new(pos, "数组元素数超过索引上限"))?;
            pos = self.skip_ws(value.end);
            match self.byte(pos) {
                Some(b',') => pos = self.skip_ws(pos + 1),
                Some(b']') => return Ok(out),
                _ => return Err(SpanIndexError::new(pos, "数组元素后应为逗号或右方括号")),
            }
        }
    }

    fn scan_value(
        self,
        pos: usize,
        depth: usize,
        count: &mut usize,
    ) -> Result<ScannedValue, SpanIndexError> {
        if depth > MAX_JSON_DEPTH {
            return Err(SpanIndexError::new(pos, "JSON 嵌套层级超过上限"));
        }
        let start = self.skip_ws(pos);
        *count = count
            .checked_add(1)
            .ok_or_else(|| SpanIndexError::new(start, "JSON 节点计数溢出"))?;

        let (end, kind) = match self.byte(start) {
            Some(b'{') => (self.scan_object(start, depth, count)?, NodeKind::Object),
            Some(b'[') => (self.scan_array(start, depth, count)?, NodeKind::Array),
            Some(b'"') => (self.scan_string(start)?, NodeKind::String),
            Some(b'-' | b'0'..=b'9') => (self.scan_number(start)?, NodeKind::Number),
            Some(b't') => (self.scan_literal(start, b"true")?, NodeKind::Bool),
            Some(b'f') => (self.scan_literal(start, b"false")?, NodeKind::Bool),
            Some(b'n') => (self.scan_literal(start, b"null")?, NodeKind::Null),
            Some(_) => return Err(SpanIndexError::new(start, "无法识别的 JSON 值")),
            None => return Err(SpanIndexError::new(start, "JSON 值意外结束")),
        };
        Ok(ScannedValue { start, end, kind })
    }

    fn scan_object(
        self,
        start: usize,
        depth: usize,
        count: &mut usize,
    ) -> Result<usize, SpanIndexError> {
        let mut pos = self.skip_ws(start + 1);
        if self.byte(pos) == Some(b'}') {
            return Ok(pos + 1);
        }
        loop {
            if self.byte(pos) != Some(b'"') {
                return Err(SpanIndexError::new(pos, "对象成员必须以字符串键开始"));
            }
            pos = self.scan_string(pos)?;
            pos = self.skip_ws(pos);
            if self.byte(pos) != Some(b':') {
                return Err(SpanIndexError::new(pos, "对象键后缺少冒号"));
            }
            let value = self.scan_value(pos + 1, depth + 1, count)?;
            pos = self.skip_ws(value.end);
            match self.byte(pos) {
                Some(b',') => pos = self.skip_ws(pos + 1),
                Some(b'}') => return Ok(pos + 1),
                _ => return Err(SpanIndexError::new(pos, "对象成员后应为逗号或右花括号")),
            }
        }
    }

    fn scan_array(
        self,
        start: usize,
        depth: usize,
        count: &mut usize,
    ) -> Result<usize, SpanIndexError> {
        let mut pos = self.skip_ws(start + 1);
        if self.byte(pos) == Some(b']') {
            return Ok(pos + 1);
        }
        loop {
            let value = self.scan_value(pos, depth + 1, count)?;
            pos = self.skip_ws(value.end);
            match self.byte(pos) {
                Some(b',') => pos = self.skip_ws(pos + 1),
                Some(b']') => return Ok(pos + 1),
                _ => return Err(SpanIndexError::new(pos, "数组元素后应为逗号或右方括号")),
            }
        }
    }

    fn scan_string(self, start: usize) -> Result<usize, SpanIndexError> {
        if self.byte(start) != Some(b'"') {
            return Err(SpanIndexError::new(start, "应为 JSON 字符串"));
        }
        let mut pos = start + 1;
        let mut raw_start = pos;
        while let Some(byte) = self.byte(pos) {
            match byte {
                b'"' => {
                    self.validate_utf8(raw_start, pos)?;
                    return Ok(pos + 1);
                }
                b'\\' => {
                    self.validate_utf8(raw_start, pos)?;
                    let escape = self
                        .byte(pos + 1)
                        .ok_or_else(|| SpanIndexError::new(pos, "字符串转义意外结束"))?;
                    match escape {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => pos += 2,
                        b'u' => {
                            let code = self.hex4(pos + 2)?;
                            pos += 6;
                            if (0xD800..=0xDBFF).contains(&code) {
                                if self.byte(pos) != Some(b'\\') || self.byte(pos + 1) != Some(b'u')
                                {
                                    return Err(SpanIndexError::new(
                                        pos,
                                        "高位代理项后缺少低位代理项",
                                    ));
                                }
                                let low = self.hex4(pos + 2)?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(SpanIndexError::new(pos + 2, "无效的低位代理项"));
                                }
                                pos += 6;
                            } else if (0xDC00..=0xDFFF).contains(&code) {
                                return Err(SpanIndexError::new(
                                    pos - 4,
                                    "低位代理项缺少高位代理项",
                                ));
                            }
                        }
                        _ => return Err(SpanIndexError::new(pos + 1, "无效的 JSON 字符串转义")),
                    }
                    raw_start = pos;
                }
                0x00..=0x1F => {
                    return Err(SpanIndexError::new(pos, "JSON 字符串包含未转义控制字符"));
                }
                _ => pos += 1,
            }
        }
        Err(SpanIndexError::new(start, "JSON 字符串缺少结束引号"))
    }

    fn scan_number(self, start: usize) -> Result<usize, SpanIndexError> {
        let mut pos = start;
        if self.byte(pos) == Some(b'-') {
            pos += 1;
        }
        match self.byte(pos) {
            Some(b'0') => pos += 1,
            Some(b'1'..=b'9') => {
                pos += 1;
                while matches!(self.byte(pos), Some(b'0'..=b'9')) {
                    pos += 1;
                }
            }
            _ => return Err(SpanIndexError::new(pos, "JSON 数字缺少整数部分")),
        }
        if self.byte(pos) == Some(b'.') {
            pos += 1;
            let fraction_start = pos;
            while matches!(self.byte(pos), Some(b'0'..=b'9')) {
                pos += 1;
            }
            if pos == fraction_start {
                return Err(SpanIndexError::new(pos, "JSON 数字小数点后缺少数字"));
            }
        }
        if matches!(self.byte(pos), Some(b'e' | b'E')) {
            pos += 1;
            if matches!(self.byte(pos), Some(b'+' | b'-')) {
                pos += 1;
            }
            let exponent_start = pos;
            while matches!(self.byte(pos), Some(b'0'..=b'9')) {
                pos += 1;
            }
            if pos == exponent_start {
                return Err(SpanIndexError::new(pos, "JSON 数字指数部分缺少数字"));
            }
        }
        Ok(pos)
    }

    fn scan_literal(self, start: usize, expected: &[u8]) -> Result<usize, SpanIndexError> {
        let end = start.saturating_add(expected.len());
        if self.bytes.get(start..end) == Some(expected) {
            Ok(end)
        } else {
            Err(SpanIndexError::new(start, "无效的 JSON 字面量"))
        }
    }

    fn hex4(self, start: usize) -> Result<u16, SpanIndexError> {
        let mut value = 0u16;
        for offset in 0..4 {
            let digit = match self.byte(start + offset) {
                Some(b'0'..=b'9') => self.bytes[start + offset] - b'0',
                Some(b'a'..=b'f') => self.bytes[start + offset] - b'a' + 10,
                Some(b'A'..=b'F') => self.bytes[start + offset] - b'A' + 10,
                _ => return Err(SpanIndexError::new(start + offset, "无效的 Unicode 转义")),
            };
            value = (value << 4) | digit as u16;
        }
        Ok(value)
    }

    fn validate_utf8(self, start: usize, end: usize) -> Result<(), SpanIndexError> {
        std::str::from_utf8(&self.bytes[start..end])
            .map(|_| ())
            .map_err(|error| {
                SpanIndexError::new(start + error.valid_up_to(), "JSON 字符串不是有效 UTF-8")
            })
    }

    fn skip_ws(self, mut pos: usize) -> usize {
        while matches!(self.byte(pos), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            pos += 1;
        }
        pos
    }

    fn byte(self, pos: usize) -> Option<u8> {
        self.bytes.get(pos).copied()
    }
}

fn pointer_depth(path: &str) -> usize {
    path.bytes().filter(|byte| *byte == b'/').count()
}

fn escape_pointer_segment(segment: &str) -> Cow<'_, str> {
    if !segment.contains(['~', '/']) {
        Cow::Borrowed(segment)
    } else {
        Cow::Owned(segment.replace('~', "~0").replace('/', "~1"))
    }
}

fn decode_pointer_segment(segment: &str) -> Option<Cow<'_, str>> {
    if !segment.contains('~') {
        return Some(Cow::Borrowed(segment));
    }
    let mut decoded = String::with_capacity(segment.len());
    let mut chars = segment.chars();
    while let Some(ch) = chars.next() {
        if ch != '~' {
            decoded.push(ch);
            continue;
        }
        match chars.next()? {
            '0' => decoded.push('~'),
            '1' => decoded.push('/'),
            _ => return None,
        }
    }
    Some(Cow::Owned(decoded))
}

/// 把刚从 `value_span` 读取的标量前缀格式化为 UI 预览。
///
/// `complete=false` 表示只读了前 `PREVIEW_READ_BYTES`，此时字符串使用增量解码，
/// 不要求尾部引号已经到达，也不会为了预览继续读取剩余大值。
pub fn scalar_preview(kind: NodeKind, raw: &[u8], complete: bool) -> String {
    match kind {
        NodeKind::String if complete => serde_json::from_slice::<String>(raw)
            .map(|value| truncate(&value, 120))
            .unwrap_or_else(|_| string_prefix_preview(raw, false)),
        NodeKind::String => string_prefix_preview(raw, true),
        NodeKind::Number | NodeKind::Bool | NodeKind::Null => {
            let mut value = String::from_utf8_lossy(raw).trim().to_string();
            if !complete {
                value.push('…');
            }
            value
        }
        NodeKind::Object | NodeKind::Array => String::new(),
    }
}

fn string_prefix_preview(raw: &[u8], force_ellipsis: bool) -> String {
    let mut out = String::new();
    let mut chars = 0usize;
    let mut pos = usize::from(raw.first() == Some(&b'"'));
    let mut closed = false;

    while pos < raw.len() && chars < 120 {
        match raw[pos] {
            b'"' => {
                closed = true;
                break;
            }
            b'\\' => {
                let Some(&escape) = raw.get(pos + 1) else {
                    break;
                };
                let decoded = match escape {
                    b'"' => Some(('"', 2)),
                    b'\\' => Some(('\\', 2)),
                    b'/' => Some(('/', 2)),
                    b'b' => Some(('\u{0008}', 2)),
                    b'f' => Some(('\u{000C}', 2)),
                    b'n' => Some(('\n', 2)),
                    b'r' => Some(('\r', 2)),
                    b't' => Some(('\t', 2)),
                    b'u' => decode_unicode_escape(raw, pos),
                    _ => None,
                };
                let Some((ch, used)) = decoded else { break };
                out.push(ch);
                chars += 1;
                pos += used;
            }
            _ => {
                let Ok(rest) = std::str::from_utf8(&raw[pos..]) else {
                    break;
                };
                let Some(ch) = rest.chars().next() else { break };
                out.push(ch);
                chars += 1;
                pos += ch.len_utf8();
            }
        }
    }
    if force_ellipsis || !closed || pos < raw.len().saturating_sub(1) {
        out.push('…');
    }
    out
}

fn decode_unicode_escape(raw: &[u8], pos: usize) -> Option<(char, usize)> {
    let high = parse_hex4(raw.get(pos + 2..pos + 6)?)?;
    if (0xD800..=0xDBFF).contains(&high) {
        if raw.get(pos + 6..pos + 8)? != b"\\u" {
            return None;
        }
        let low = parse_hex4(raw.get(pos + 8..pos + 12)?)?;
        if !(0xDC00..=0xDFFF).contains(&low) {
            return None;
        }
        let codepoint = 0x10000 + (((high - 0xD800) as u32) << 10) + (low - 0xDC00) as u32;
        char::from_u32(codepoint).map(|ch| (ch, 12))
    } else if (0xDC00..=0xDFFF).contains(&high) {
        None
    } else {
        char::from_u32(high as u32).map(|ch| (ch, 6))
    }
}

fn parse_hex4(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 {
        return None;
    }
    bytes.iter().try_fold(0u16, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        Some((value << 4) | digit as u16)
    })
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        let cut: String = value.chars().take(max).collect();
        format!("{cut}…")
    }
}

// ─────────────────────────────────────────────────────────────────────
// 响应存储（spool + LRU 索引缓存）
// ─────────────────────────────────────────────────────────────────────

/// 响应体摘要。这是**唯一会过 IPC 的响应信息**，只有几百字节。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSummary {
    pub id: Uuid,
    pub size_bytes: u64,
    pub tier: RenderTier,
    pub syntax_highlight: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// JSON 是否已建索引（SummaryOnly 分级下不自动建）
    pub indexed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_nodes: Option<usize>,
    /// SpanIndex 的近似常驻堆内存，不含响应体 mmap/文件。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_bytes: Option<usize>,
    pub timing: Timing,

    // ── HTTP 语义 ───────────────────────────────────────────────────
    //
    // 为什么加在这里，而不是把 `model::ResponseMeta` 直接序列化给前端：
    // `ResponseMeta` 有 `body_path: PathBuf`。把它送过 IPC 就等于把 spool 的真实
    // 路径交给 WebView，而前端本来连文件系统权限都没有（PRD §10.2 能力最小化）。
    // 所以 `ResponseMeta` 留在 Rust 侧，这里只挑前端渲染真正需要的字段。
    //
    // 全部 `Option`/默认空：Spike 的合成响应没有 HTTP 语义，那时为 `None`，
    // UI 应显示「不可用」而不是编一个 200 出来。
    /// HTTP 状态码。`None` = 非 HTTP 来源（合成响应）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_version: Option<String>,
    /// 用 `Vec` 而非 `Map`：HTTP 允许同名头重复（`Set-Cookie` 是常态），
    /// 用 map 会静默丢掉重复项。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    /// 超过 `MAX_BODY_BYTES` 被截断
    #[serde(default)]
    pub truncated: bool,
}

impl ResponseSummary {
    /// 无 HTTP 语义的摘要（Spike 合成响应）。
    ///
    /// 索引相关字段留给调用方填 —— 那要查 `ResponseStore` 的当前状态，
    /// 而索引可以在响应落盘之后才建，不能在这里定死。
    pub fn without_http(id: Uuid, size_bytes: u64, timing: Timing) -> Self {
        let tier = RenderTier::from_size(size_bytes);
        Self {
            id,
            size_bytes,
            tier,
            syntax_highlight: tier.syntax_highlight(),
            content_type: None,
            indexed: false,
            total_nodes: None,
            index_bytes: None,
            timing,
            status: None,
            status_text: None,
            http_version: None,
            headers: Vec::new(),
            charset: None,
            truncated: false,
        }
    }

    /// 从 Rust 侧的 `ResponseMeta` 投影出可过 IPC 的摘要（丢掉 `body_path`）。
    pub fn from_meta(meta: &crate::model::ResponseMeta) -> Self {
        let tier = RenderTier::from_size(meta.size_bytes);
        Self {
            id: meta.id,
            size_bytes: meta.size_bytes,
            tier,
            syntax_highlight: tier.syntax_highlight(),
            content_type: meta.content_type.clone(),
            indexed: false,
            total_nodes: None,
            index_bytes: None,
            timing: meta.timing,
            status: Some(meta.status),
            status_text: Some(meta.status_text.clone()),
            http_version: Some(meta.http_version.clone()),
            headers: meta.headers.clone(),
            charset: meta.charset.clone(),
            truncated: meta.truncated,
        }
    }
}

struct CacheEntry {
    index: Arc<SpanIndex>,
    bytes: usize,
}

struct IndexCache {
    entries: HashMap<Uuid, CacheEntry>,
    order: VecDeque<Uuid>,
    total_bytes: usize,
    max_bytes: usize,
}

#[derive(Clone)]
pub struct ResponseStore {
    inner: Arc<Mutex<IndexCache>>,
}

impl Default for ResponseStore {
    fn default() -> Self {
        Self::with_budget(DEFAULT_INDEX_CACHE_BYTES)
    }
}

impl ResponseStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_budget(max_bytes: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(IndexCache {
                entries: HashMap::new(),
                order: VecDeque::new(),
                total_bytes: 0,
                max_bytes,
            })),
        }
    }

    /// 锁中毒时仍然返回数据(`Mutex::lock` 失败意味着持锁线程 panic)。
    /// 与 `src-tauri/src/state.rs` 的 `in_flight` / `meta` 处理风格保持一致。
    /// `panic = "abort"` 的 release 下,这里如果崩了就是真的 abort 整进程,
    /// 所以宁可读到中毒后的脏状态也不能让产品闪退。
    fn lock_inner(&self) -> std::sync::MutexGuard<'_, IndexCache> {
        self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("ResponseStore 锁中毒,采用 into_inner() 恢复");
            poisoned.into_inner()
        })
    }

    pub fn insert_index(&self, id: Uuid, index: SpanIndex) {
        let bytes = index.estimated_bytes();
        let mut cache = self.lock_inner();
        if let Some(previous) = cache.entries.remove(&id) {
            cache.total_bytes = cache.total_bytes.saturating_sub(previous.bytes);
            cache.order.retain(|cached| *cached != id);
        }
        cache.entries.insert(
            id,
            CacheEntry {
                index: Arc::new(index),
                bytes,
            },
        );
        cache.order.push_back(id);
        cache.total_bytes = cache.total_bytes.saturating_add(bytes);

        // 单个最新索引即使超过预算也保留，让显式「解析」不会刚完成就失效。
        while cache.total_bytes > cache.max_bytes && cache.entries.len() > 1 {
            let Some(oldest) = cache.order.pop_front() else {
                break;
            };
            if let Some(evicted) = cache.entries.remove(&oldest) {
                cache.total_bytes = cache.total_bytes.saturating_sub(evicted.bytes);
            }
        }
    }

    pub fn index(&self, id: Uuid) -> Option<Arc<SpanIndex>> {
        let mut cache = self.lock_inner();
        let index = cache.entries.get(&id).map(|entry| entry.index.clone());
        if index.is_some() {
            cache.order.retain(|cached| *cached != id);
            cache.order.push_back(id);
        }
        index
    }

    /// 标签进入 Cold 状态时调用；Arc 允许正在执行的分页请求安全完成。
    pub fn drop_index(&self, id: Uuid) {
        let mut cache = self.lock_inner();
        if let Some(removed) = cache.entries.remove(&id) {
            cache.total_bytes = cache.total_bytes.saturating_sub(removed.bytes);
        }
        cache.order.retain(|cached| *cached != id);
    }

    pub fn indexed_count(&self) -> usize {
        self.lock_inner().entries.len()
    }

    pub fn index_bytes(&self) -> usize {
        self.lock_inner().total_bytes
    }
}

/// 生成用于压测的合成 JSON 载荷（Spike S2）。
///
/// 结构刻意做得像真实列表接口：外层包一层 `code/message/data`，
/// `data.list` 是对象数组，每个对象有嵌套结构。
pub fn synth_json_payload(target_bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(target_bytes + 1024);
    out.extend_from_slice(b"{\"code\":0,\"message\":\"ok\",\"data\":{\"total\":0,\"list\":[");
    let mut i = 0usize;
    let mut first = true;
    while out.len() < target_bytes {
        if !first {
            out.push(b',');
        }
        first = false;
        let item = format!(
            r#"{{"id":{i},"name":"用户-{i}","email":"user{i}@example.com","active":{active},"score":{score}.5,"tags":["alpha","beta","gamma"],"profile":{{"city":"杭州","zip":"3100{z}","nested":{{"level":3,"note":"这是一段用于填充体积的中文描述文本，包含多字节字符以贴近真实场景"}}}}}}"#,
            i = i,
            active = i % 2 == 0,
            score = i % 100,
            z = i % 10
        );
        out.extend_from_slice(item.as_bytes());
        i += 1;
    }
    out.extend_from_slice(b"]}}");
    out
}

// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("apogee-test-{name}-{}", Uuid::new_v4()))
    }

    fn page(
        index: &SpanIndex,
        raw: &[u8],
        path: &str,
        offset: usize,
        limit: usize,
    ) -> Vec<TreeNode> {
        index
            .children(path, offset, limit)
            .into_iter()
            .map(|mut indexed| {
                if let Some(span) = indexed.value_span {
                    let bytes = &raw[span.start as usize..span.end as usize];
                    indexed.node.preview = Some(scalar_preview(indexed.node.kind, bytes, true));
                }
                indexed.node
            })
            .collect()
    }

    #[test]
    fn spool_round_trip_and_range() {
        let spool = Spool::new(tmp_root("spool")).unwrap();
        let id = Uuid::new_v4();
        let data = b"0123456789abcdef".to_vec();
        let n = spool.write_stream(id, [&data[..8], &data[8..]]).unwrap();
        assert_eq!(n, 16);
        assert_eq!(spool.size_of(id).unwrap(), 16);
        assert_eq!(spool.read_range(id, 0, 4).unwrap(), b"0123");
        assert_eq!(spool.read_range(id, 10, 4).unwrap(), b"abcd");
        assert!(spool.read_range(id, 999, 4).unwrap().is_empty());
        assert_eq!(spool.read_range(id, 12, 100).unwrap(), b"cdef");
        spool.remove(id).unwrap();
    }

    #[test]
    fn span_index_pages_children_without_value_tree() {
        let raw = br#"{"code":0,"data":{"list":[{"a":1},{"a":2},{"a":3}],"n":null}}"#;
        let index = SpanIndex::parse(raw).unwrap();

        let root = page(&index, raw, "", 0, 10);
        assert_eq!(root.len(), 2);
        assert_eq!(root[0].key, "code");
        assert_eq!(root[0].kind, NodeKind::Number);
        assert_eq!(root[0].preview.as_deref(), Some("0"));
        assert_eq!(root[1].key, "data");
        assert_eq!(root[1].kind, NodeKind::Object);
        assert_eq!(root[1].child_count, 2);

        let list = page(&index, raw, "/data/list", 1, 1);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].path, "/data/list/1");
        assert_eq!(list[0].kind, NodeKind::Object);
        assert!(page(&index, raw, "/data/n", 0, 10).is_empty());
    }

    #[test]
    fn handles_escaped_keys_unicode_and_all_scalar_kinds() {
        let raw = br#"{"a/b":{"~key":"line\n\u4e2d\ud83d\ude00"},"number":-12.5e+2,"yes":true,"no":false,"nil":null,"empty":[]}"#;
        let index = SpanIndex::parse(raw).unwrap();
        let nested = page(&index, raw, "/a~1b", 0, 10);
        assert_eq!(nested[0].path, "/a~1b/~0key");
        assert_eq!(nested[0].preview.as_deref(), Some("line\n中😀"));
        assert!(index.span("/a~1b/~0key").is_some());

        let root = page(&index, raw, "", 0, 20);
        assert_eq!(
            root.iter()
                .find(|node| node.key == "number")
                .unwrap()
                .preview
                .as_deref(),
            Some("-12.5e+2")
        );
        assert_eq!(
            root.iter().find(|node| node.key == "yes").unwrap().kind,
            NodeKind::Bool
        );
        assert_eq!(
            root.iter()
                .find(|node| node.key == "nil")
                .unwrap()
                .preview
                .as_deref(),
            Some("null")
        );
        assert_eq!(
            root.iter()
                .find(|node| node.key == "empty")
                .unwrap()
                .child_count,
            0
        );
    }

    #[test]
    fn rejects_invalid_json_and_surrogates() {
        for raw in [
            br#"{"a":1,}"#.as_slice(),
            br#"[01]"#.as_slice(),
            br#"{"a":"\ud800"}"#.as_slice(),
            br#"true false"#.as_slice(),
        ] {
            assert!(
                SpanIndex::parse(raw).is_err(),
                "应拒绝 {}",
                String::from_utf8_lossy(raw)
            );
        }
    }

    #[test]
    fn long_string_preview_is_bounded() {
        let value = format!("\"{}\"", "中".repeat(1_000));
        let prefix = &value.as_bytes()[..PREVIEW_READ_BYTES.min(value.len() as u64) as usize];
        let preview = scalar_preview(NodeKind::String, prefix, prefix.len() == value.len());
        assert!(preview.chars().count() <= 121);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn span_index_metadata_stays_below_three_times_body() {
        let bytes = synth_json_payload(1024 * 1024);
        let index = SpanIndex::parse(&bytes).expect("生成的载荷必须是合法 JSON");
        assert!(index.total_nodes() > 100);
        assert!(
            index.estimated_bytes() < bytes.len() * 3,
            "index={} body={}",
            index.estimated_bytes(),
            bytes.len()
        );
    }

    #[test]
    fn response_store_evicts_lru_and_cold_drop_releases_entry() {
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let first = SpanIndex::parse(br#"{"a":1}"#).unwrap();
        let budget = first.estimated_bytes() + 1;
        let store = ResponseStore::with_budget(budget);
        store.insert_index(first_id, first);
        store.insert_index(second_id, SpanIndex::parse(br#"{"b":2}"#).unwrap());
        assert!(store.index(first_id).is_none());
        assert!(store.index(second_id).is_some());
        store.drop_index(second_id);
        assert_eq!(store.indexed_count(), 0);
        assert_eq!(store.index_bytes(), 0);
    }

    #[test]
    fn synth_payload_is_valid_json_at_target_size() {
        let bytes = synth_json_payload(256 * 1024);
        assert!(bytes.len() >= 256 * 1024);
        let index = SpanIndex::parse(&bytes).expect("生成的载荷必须是合法 JSON");
        assert!(index.total_nodes() > 100);
    }
}
