//! 工作区与集合的持久化数据契约。
//!
//! 文件读写由桌面端负责；这些类型放在 core 里，保证 Tauri 命令、未来 CLI
//! 和公开的 YAML 格式共享同一份模型。

use crate::request::{AuthConfig, RequestSettings, RequestSpec};
use crate::vars::Environment;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 集合文件格式版本。v1 只增字段，不改变已有字段语义。
pub const WORKSPACE_FORMAT_VERSION: u32 = 1;

/// `apogee.yaml` 的工作区清单。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    pub apogee: u32,
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub collections: Vec<CollectionEntry>,
    #[serde(default)]
    pub environments: Vec<EnvironmentEntry>,
}

/// 清单中的环境入口。实际变量值单独保存在 environments/<id>.yaml。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentEntry {
    pub id: Uuid,
    pub name: String,
}

/// 清单中的集合入口。目录名使用稳定 ID，重命名不会移动文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionEntry {
    pub id: Uuid,
    pub name: String,
    pub directory: String,
}

/// 集合或文件夹目录下的 `_collection.yaml` / `_folder.yaml`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerDocument {
    pub apogee: u32,
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub settings: RequestSettings,
}

/// 集合或文件夹的可编辑配置投影。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceContainer {
    pub id: Uuid,
    pub name: String,
    pub auth: AuthConfig,
    pub settings: RequestSettings,
}

/// Rust 返回给前端的扁平节点类型。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceNodeKind {
    Collection,
    Folder,
    Request,
}

/// 集合树的一行。没有 `children`，前端可以直接交给虚拟滚动组件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceNode {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub depth: usize,
    pub name: String,
    pub method: Option<String>,
    pub is_folder: bool,
    pub expanded: bool,
    pub kind: WorkspaceNodeKind,
}

/// `workspace_load` 的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub id: Uuid,
    pub name: String,
    pub nodes: Vec<WorkspaceNode>,
    pub environments: Vec<Environment>,
}

/// 前端打开请求时使用的轻量投影。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRequest {
    pub id: Uuid,
    pub name: String,
    pub spec: RequestSpec,
}

/// 独立请求 YAML 文件的格式。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedRequest {
    pub apogee: u32,
    pub id: Uuid,
    pub name: String,
    #[serde(flatten)]
    pub spec: RequestSpec,
}

impl PersistedRequest {
    pub fn as_ipc(&self) -> WorkspaceRequest {
        WorkspaceRequest {
            id: self.id,
            name: self.name.clone(),
            spec: self.spec.clone(),
        }
    }
}
