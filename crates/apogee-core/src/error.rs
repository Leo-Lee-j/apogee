//! 统一错误契约（03 文档 §3.3）。
//!
//! 设计要点：错误必须携带**可操作建议**（`hints`），前端据此渲染
//! 02 文档 §8 的三段式错误提示：发生了什么 → 为什么 → 现在能做什么。
//! 不允许出现「未知错误」这类零信息量文案。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    Dns,
    Connect,
    Tls,
    Timeout,
    Http,
    Io,
    Parse,
    Script,
    /// 变量未定义或解析失败
    Var,
}

/// 一条可操作建议。`action` 是前端可路由的动作 id，例如
/// `"settings.tls.disable_for_collection"`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hint {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

impl Hint {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: None,
        }
    }

    pub fn with_action(label: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: Some(action.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppError {
    pub kind: ErrorKind,
    /// 已本地化的简述
    pub message: String,
    /// 技术细节，前端可折叠展示
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 可操作建议。**空数组是设计缺陷**，评审时应被指出。
    pub hints: Vec<Hint>,
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: None,
            hints: Vec::new(),
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn hint(mut self, hint: Hint) -> Self {
        self.hints.push(hint);
        self
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for AppError {}
