//! Apogee 领域逻辑核心库。
//!
//! # 架构约束（03 文档 §2.2）
//!
//! 本 crate 承载所有 `O(响应体大小)` 与 `O(集合规模)` 的计算。前端永远不持有完整
//! 响应字符串，也不做树结构递归构造。这是打破 Electron 系客户端性能瓶颈的关键。
//!
//! 本 crate **不依赖 Tauri**，以便 v0.5 的 CLI 直接复用。

mod auth;
pub mod automation;
pub mod cookies;
pub mod curl;
pub mod error;
pub mod http;
pub mod model;
pub mod oauth2;
pub mod postman;
pub mod public;
pub mod request;
pub mod response;
pub mod update;
pub mod vars;
pub mod workspace;
pub mod yaml;
pub mod yapi;

pub use error::{AppError, ErrorKind, Hint};

/// 集合文件格式版本（决策 D4）。
///
/// v1.0 起承诺 Format v1 稳定：新字段只增不改语义。
pub const FORMAT_VERSION: u32 = 1;
