//! 核心数据模型（03 文档 §5）。
//!
//! 骨架阶段只落地 Spike 与首屏需要的类型。完整模型（Request / Body / AuthConfig /
//! Scripts / Variable ...）按 03 文档 §5.2 逐步补齐。
//!
//! 约定：所有 `Option` 型的 settings 字段，`None` 表示**继承上级**，实现
//! 「集合 → 文件夹 → 请求」三级配置继承。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Protocol {
    Http,
    GraphQl,
    WebSocket,
    Sse,
    Grpc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValueKind {
    Text,
    File,
    Secret,
}

/// 参数 / Header 行。`enabled` 对应 UI 上的勾选框 —— 调试时最常见的动作是
/// 临时去掉一个参数，而不是删除它（02 文档 §4.2）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyValue {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_value_kind")]
    pub kind: ValueKind,
}

fn default_true() -> bool {
    true
}

fn default_value_kind() -> ValueKind {
    ValueKind::Text
}

/// 请求耗时分解。用于 02 文档 §4.4 的时序瀑布图，也是「性能自检」面板的数据来源。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    pub dns_ms: f64,
    pub tcp_ms: f64,
    pub tls_ms: f64,
    pub request_ms: f64,
    pub ttfb_ms: f64,
    pub download_ms: f64,
    pub total_ms: f64,
}

/// 响应元信息。
///
/// **关键设计**：这里**不包含响应体**。body 落在磁盘 spool（`body_path`），
/// 前端通过 `apogee://` 自定义协议按 Range 读取，绝不整体过 IPC。
/// 这是「10MB JSON 首屏 < 300ms / 100MB 不崩」的前提（03 文档 §3.1、§4.3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseMeta {
    pub id: Uuid,
    pub status: u16,
    pub status_text: String,
    pub http_version: String,
    pub headers: Vec<(String, String)>,
    pub size_bytes: u64,
    pub body_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    pub timing: Timing,
    pub from_cache: bool,
    /// 超过上限被截断
    pub truncated: bool,
}

/// 响应体渲染分级（02 文档 §4.4）。
///
/// **唯一的高亮阈值是 2 MB**，三份设计文档统一按此口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderTier {
    /// < 512 KB：全功能，树视图 + 全量高亮 + 折叠 + 搜索
    Full,
    /// 512 KB ～ 2 MB：虚拟滚动 + 视口内高亮
    Virtualized,
    /// 2 MB ～ 50 MB：关闭语法高亮，树视图需手动解析
    NoHighlight,
    /// > 50 MB：只显示摘要 + 保存到文件 / 外部打开 / 头部流式预览
    SummaryOnly,
}

pub const TIER_FULL_MAX: u64 = 512 * 1024;
pub const TIER_HIGHLIGHT_MAX: u64 = 2 * 1024 * 1024;
pub const TIER_RENDER_MAX: u64 = 50 * 1024 * 1024;

impl RenderTier {
    pub fn from_size(size_bytes: u64) -> Self {
        match size_bytes {
            n if n < TIER_FULL_MAX => Self::Full,
            n if n < TIER_HIGHLIGHT_MAX => Self::Virtualized,
            n if n < TIER_RENDER_MAX => Self::NoHighlight,
            _ => Self::SummaryOnly,
        }
    }

    /// 是否开启语法高亮。阈值 2MB，见 02 文档 §4.4。
    pub fn syntax_highlight(self) -> bool {
        matches!(self, Self::Full | Self::Virtualized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_boundaries_match_design_doc() {
        assert_eq!(RenderTier::from_size(0), RenderTier::Full);
        assert_eq!(RenderTier::from_size(512 * 1024 - 1), RenderTier::Full);
        assert_eq!(RenderTier::from_size(512 * 1024), RenderTier::Virtualized);
        assert_eq!(
            RenderTier::from_size(2 * 1024 * 1024 - 1),
            RenderTier::Virtualized
        );
        // 2MB 是唯一的高亮阈值
        assert_eq!(
            RenderTier::from_size(2 * 1024 * 1024),
            RenderTier::NoHighlight
        );
        assert_eq!(
            RenderTier::from_size(50 * 1024 * 1024 - 1),
            RenderTier::NoHighlight
        );
        assert_eq!(
            RenderTier::from_size(50 * 1024 * 1024),
            RenderTier::SummaryOnly
        );
    }

    #[test]
    fn highlight_cutoff_is_2mb() {
        assert!(RenderTier::from_size(2 * 1024 * 1024 - 1).syntax_highlight());
        assert!(!RenderTier::from_size(2 * 1024 * 1024).syntax_highlight());
    }
}
