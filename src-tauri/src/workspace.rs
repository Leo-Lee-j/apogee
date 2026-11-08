//! 本地优先工作区的文件仓库。
//!
//! 当前默认工作区位于 Tauri app data 目录：
//!
//! ```text
//! workspace/
//! ├── apogee.yaml
//! └── collections/<collection-id>/
//!     ├── _collection.yaml
//!     ├── <request-id>.yaml
//!     └── <folder-id>/
//!         ├── _folder.yaml
//!         └── <request-id>.yaml
//! ```
//!
//! 目录名只使用稳定 UUID，用户重命名不会移动路径；可读名称全部写在 YAML 中。

use anyhow::{bail, Context, Result};
use apogee_core::model::KeyValue;
use apogee_core::public::{
    PublicActualRequest, PublicError, PublicRequestPreview, PublicResponseSummary,
};
use apogee_core::request::{AuthConfig, Body, RequestSettings, RequestSpec};
#[cfg(test)]
use apogee_core::response::ResponseSummary;
use apogee_core::vars::Environment;
use apogee_core::workspace::{
    CollectionEntry, ContainerDocument, DebugCase, EnvironmentEntry, PersistedRequest,
    WorkspaceContainer, WorkspaceManifest, WorkspaceNode, WorkspaceNodeKind, WorkspaceRequest,
    WorkspaceSnapshot, WORKSPACE_FORMAT_VERSION,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const COLLECTION_MARKER: &str = "_collection.yaml";
const FOLDER_MARKER: &str = "_folder.yaml";
const ENVIRONMENT_DIR: &str = "environments";
const HISTORY_DIR: &str = "history";
const HISTORY_LIMIT: usize = 200;
const TRANSACTION_DIR: &str = ".apogee-tx";
const TRANSACTION_VERSION: u32 = 1;

/// 可恢复多文件操作的持久化标记。journal 只保存由 UUID 推导的对象身份，绝不接受
/// 外部路径；恢复路径因此不会因损坏/手写 journal 删除工作区外的任意文件。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceTransactionJournal {
    version: u32,
    operation: WorkspaceTransactionOperation,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum WorkspaceTransactionOperation {
    EnvironmentCreate { environment_id: Uuid },
    EnvironmentDelete { environment_id: Uuid },
    CollectionDelete { collection_id: Uuid },
    CollectionRename { collection_id: Uuid },
}

impl WorkspaceTransactionOperation {
    fn journal_file_name(&self) -> String {
        match self {
            Self::EnvironmentCreate { environment_id } => {
                format!("environment-create-{environment_id}.json")
            }
            Self::EnvironmentDelete { environment_id } => {
                format!("environment-delete-{environment_id}.json")
            }
            Self::CollectionDelete { collection_id } => {
                format!("collection-delete-{collection_id}.json")
            }
            Self::CollectionRename { collection_id } => {
                format!("collection-rename-{collection_id}.json")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YapiWorkspaceImportResult {
    pub collection_id: Uuid,
    pub collection_name: String,
    pub imported_requests: usize,
    pub imported_folders: usize,
    pub imported_environments: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: Uuid,
    pub created_at_ms: u64,
    #[serde(default)]
    pub debug_case_id: Option<Uuid>,
    #[serde(default)]
    pub environment_id: Option<Uuid>,
    pub method: String,
    pub url: String,
    /// `None` 表示请求在收到 HTTP 响应前失败，前端显示为 ERR。
    pub status: Option<u16>,
    pub size_bytes: u64,
    pub duration_ms: f64,
}

/// 一条运行记录恰有一个结果：成功时有 `response` 和响应体文件，失败时有 `error`
/// 及可选的最终请求快照。该记录只使用公开 DTO，不能包含可执行请求、请求体或
/// 认证凭据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: Uuid,
    pub created_at_ms: u64,
    #[serde(default)]
    pub debug_case_id: Option<Uuid>,
    #[serde(default)]
    pub environment_id: Option<Uuid>,
    #[serde(default)]
    pub spec: PublicRequestPreview,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<PublicResponseSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<PublicError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_request: Option<PublicActualRequest>,
    /// 失败时没有 `PublicResponseSummary`，因此把从发送到终结事件的耗时单独保存。
    #[serde(default)]
    pub duration_ms: f64,
}

impl HistoryRecord {
    /// Old YAML may contain raw fields that serde ignores while reading the public DTO. Re-run
    /// the DTO sanitizers before any history value is returned to a caller. The raw preview must
    /// be consulted first because it is the only compatibility source for old actual snapshots
    /// that predate `ActualRequest` sensitivity metadata.
    fn redacted(mut self) -> Self {
        let spec = std::mem::take(&mut self.spec);
        self.response = self
            .response
            .map(|response| spec.redact_response_summary(response));
        self.error = self.error.map(PublicError::redacted);
        self.actual_request = self
            .actual_request
            .map(|actual_request| spec.redact_actual_request(actual_request));
        self.spec = spec.redacted();
        self
    }
}

/// 将未收到 HTTP 响应的运行写入历史所需的领域输入。
///
/// 它与持久化后的 [`HistoryRecord`] 分离：仓储负责生成记录 id 并固定失败记录
/// 没有响应体的约束，调用方只能提供已净化的公开 DTO。
#[derive(Debug)]
pub struct FailedHistoryWriteInput {
    pub created_at_ms: u64,
    pub spec: PublicRequestPreview,
    pub error: PublicError,
    pub actual_request: Option<PublicActualRequest>,
    pub duration_ms: f64,
    pub debug_case_id: Option<Uuid>,
    pub environment_id: Option<Uuid>,
}

pub struct WorkspaceRepository {
    root: PathBuf,
    runtime_root: Option<PathBuf>,
}

struct LocatedNode {
    node: WorkspaceNode,
    path: PathBuf,
}

impl WorkspaceRepository {
    pub fn create_example(root: impl Into<PathBuf>) -> Result<Self> {
        let repository = Self::create(root, "示例工作区".to_string())?;
        let snapshot = repository.create_collection("示例 API 集合".to_string())?;
        let collection = snapshot
            .nodes
            .iter()
            .find(|n| matches!(n.kind, WorkspaceNodeKind::Collection))
            .map(|n| n.id)
            .context("示例集合创建失败")?;
        let folder_snapshot = repository.create_folder(collection, "请求类型示例".to_string())?;
        let folder = folder_snapshot
            .nodes
            .iter()
            .find(|n| n.name == "请求类型示例")
            .map(|n| n.id)
            .context("示例文件夹创建失败")?;
        let methods = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
        for method in methods {
            let request = repository.create_request(folder, format!("{method} 请求"))?;
            let mut spec = request.spec;
            spec.method = method.to_string();
            spec.url = "https://httpbin.org/anything".to_string();
            repository.save_request(request.id, request.name, spec)?;
        }
        let bodies = [
            (
                "JSON 请求体",
                Body::Json {
                    content: "{\"hello\":\"world\"}".to_string(),
                },
            ),
            (
                "文本请求体",
                Body::Text {
                    content: "hello".to_string(),
                    content_type: "text/plain".to_string(),
                },
            ),
            (
                "表单请求体",
                Body::FormData {
                    rows: vec![KeyValue {
                        enabled: true,
                        key: "name".to_string(),
                        value: "Apogee".to_string(),
                        description: None,
                        kind: apogee_core::model::ValueKind::Text,
                    }],
                },
            ),
            (
                "URL 编码请求体",
                Body::UrlEncoded {
                    rows: vec![KeyValue {
                        enabled: true,
                        key: "name".to_string(),
                        value: "Apogee".to_string(),
                        description: None,
                        kind: apogee_core::model::ValueKind::Text,
                    }],
                },
            ),
            (
                "GraphQL 请求体",
                Body::GraphQl {
                    query: "query { anything }".to_string(),
                    variables: "{}".to_string(),
                    operation_name: None,
                },
            ),
            (
                "二进制请求体模板",
                Body::Binary {
                    path: PathBuf::new(),
                },
            ),
        ];
        for (name, body) in bodies {
            let request = repository.create_request(folder, name.to_string())?;
            let mut spec = request.spec;
            spec.method = "POST".to_string();
            spec.url = "https://httpbin.org/anything".to_string();
            spec.body = body;
            repository.save_request(request.id, request.name, spec)?;
        }
        let auths = [
            (
                "Basic 认证",
                AuthConfig::Basic {
                    username: "demo".to_string(),
                    password: "password".to_string(),
                },
            ),
            (
                "Bearer 认证",
                AuthConfig::Bearer {
                    token: "demo-token".to_string(),
                },
            ),
            (
                "API Key 认证",
                AuthConfig::ApiKey {
                    key: "X-API-Key".to_string(),
                    value: "demo-key".to_string(),
                    location: apogee_core::request::ApiKeyIn::Header,
                },
            ),
        ];
        for (name, auth) in auths {
            let request = repository.create_request(folder, name.to_string())?;
            let mut spec = request.spec;
            spec.method = "GET".to_string();
            spec.url = "https://httpbin.org/anything".to_string();
            spec.auth = auth;
            repository.save_request(request.id, request.name, spec)?;
        }
        Ok(repository)
    }
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.join("apogee.yaml").is_file() {
            bail!("所选目录不是 Apogee 工作区：缺少 apogee.yaml");
        }
        let repository = Self {
            root,
            runtime_root: None,
        };
        repository.read_manifest()?;
        Ok(repository)
    }

    pub fn create(root: impl Into<PathBuf>, name: String) -> Result<Self> {
        let root = root.into();
        if root.exists() && fs::read_dir(&root)?.next().is_some() {
            bail!("新工作区必须使用空目录");
        }
        fs::create_dir_all(root.join("collections"))?;
        fs::create_dir_all(root.join(ENVIRONMENT_DIR))?;
        let repository = Self {
            root,
            runtime_root: None,
        };
        repository.create_default_workspace(name)?;
        Ok(repository)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 在取得 workspace OS 写锁后恢复或回滚未完成的工作区事务。
    ///
    /// 调用方必须在任何 snapshot、registry 更新或 runtime migration 前调用它。恢复不
    /// 修改已提交 manifest 的业务含义：create 的 manifest 尚未引用对象时删除孤儿；
    /// delete 的 manifest 仍引用对象时把 tombstone 放回原位。
    pub(crate) fn recover_pending_transactions(&self) -> Result<()> {
        let directory = self.transaction_directory();
        if !directory.exists() {
            return Ok(());
        }
        if !directory.is_dir() {
            bail!("工作区事务目录不是目录：{}", directory.display());
        }

        let manifest = self.read_manifest()?;
        let mut journals = fs::read_dir(&directory)?.collect::<std::result::Result<Vec<_>, _>>()?;
        journals.sort_by_key(|entry| entry.file_name());
        for entry in journals {
            if !entry.file_type()?.is_file()
                || entry.path().extension().and_then(|item| item.to_str()) != Some("json")
            {
                continue;
            }
            let path = entry.path();
            let journal: WorkspaceTransactionJournal = serde_json::from_slice(&fs::read(&path)?)
                .with_context(|| format!("读取工作区事务 journal 失败：{}", path.display()))?;
            if journal.version != TRANSACTION_VERSION {
                bail!(
                    "不支持的工作区事务 journal 版本 {}：{}",
                    journal.version,
                    path.display()
                );
            }
            match journal.operation {
                WorkspaceTransactionOperation::EnvironmentCreate { environment_id } => {
                    self.recover_environment_create(&manifest, environment_id, &path)?;
                }
                WorkspaceTransactionOperation::EnvironmentDelete { environment_id } => {
                    self.recover_environment_delete(&manifest, environment_id, &path)?;
                }
                WorkspaceTransactionOperation::CollectionDelete { collection_id } => {
                    self.recover_collection_delete(&manifest, collection_id, &path)?;
                }
                WorkspaceTransactionOperation::CollectionRename { collection_id } => {
                    self.recover_collection_rename(&manifest, collection_id, &path)?;
                }
            }
        }
        Ok(())
    }

    fn transaction_directory(&self) -> PathBuf {
        self.root.join(TRANSACTION_DIR)
    }

    fn begin_transaction(&self, operation: WorkspaceTransactionOperation) -> Result<PathBuf> {
        let path = self
            .transaction_directory()
            .join(operation.journal_file_name());
        if path.exists() {
            bail!("检测到未恢复的工作区事务：{}", path.display());
        }
        let journal = WorkspaceTransactionJournal {
            version: TRANSACTION_VERSION,
            operation,
        };
        let payload = serde_json::to_vec_pretty(&journal)?;
        crate::persistence::atomic_write(&path, &payload)
            .with_context(|| format!("写入工作区事务 journal 失败：{}", path.display()))?;
        Ok(path)
    }

    fn finish_transaction(&self, path: &Path) -> Result<()> {
        remove_file_if_exists(path)
            .with_context(|| format!("清理工作区事务 journal 失败：{}", path.display()))
    }

    fn environment_create_staging_path(&self, id: Uuid) -> PathBuf {
        self.root
            .join(ENVIRONMENT_DIR)
            .join(format!(".environment-create-{id}.staging"))
    }

    fn environment_delete_tombstone_path(&self, id: Uuid) -> PathBuf {
        self.root
            .join(ENVIRONMENT_DIR)
            .join(format!(".environment-delete-{id}.tombstone"))
    }

    fn recover_environment_create(
        &self,
        manifest: &WorkspaceManifest,
        id: Uuid,
        journal: &Path,
    ) -> Result<()> {
        let target = self.environment_path(id);
        let staging = self.environment_create_staging_path(id);
        let committed = manifest.environments.iter().any(|entry| entry.id == id);
        if committed {
            if !target.is_file() {
                bail!("环境创建事务已提交但目标文件缺失：{}", target.display());
            }
            remove_file_if_exists(&staging)?;
        } else {
            // manifest 未引用该 UUID，说明 publish 在提交点前中断；删除 stage 或已发布
            // orphan 后再移除 journal，避免它在下次打开时再次成为可见半成品。
            remove_file_if_exists(&staging)?;
            remove_file_if_exists(&target)?;
        }
        self.finish_transaction(journal)
    }

    fn recover_environment_delete(
        &self,
        manifest: &WorkspaceManifest,
        id: Uuid,
        journal: &Path,
    ) -> Result<()> {
        let target = self.environment_path(id);
        let tombstone = self.environment_delete_tombstone_path(id);
        let committed = manifest.environments.iter().all(|entry| entry.id != id);
        if committed {
            // manifest 是删除提交点。若进程在清理文件前中断，目标文件或 tombstone 都
            // 已不再被 manifest 引用，可安全移除。
            remove_file_if_exists(&tombstone)?;
            remove_file_if_exists(&target)?;
        } else if target.exists() {
            if tombstone.exists() {
                bail!(
                    "环境删除恢复发现目标与 tombstone 同时存在：{}",
                    tombstone.display()
                );
            }
        } else if tombstone.exists() {
            fs::rename(&tombstone, &target)
                .with_context(|| format!("恢复未提交的环境删除失败：{}", target.display()))?;
        } else {
            bail!(
                "环境删除事务未提交但目标和 tombstone 均缺失：{}",
                target.display()
            );
        }
        self.finish_transaction(journal)
    }

    /// 事务只记录 UUID；清理路径必须能由标准布局推导，并且不能穿过符号链接。
    /// 非标准目录仍可读取，但不能猜测它的删除恢复路径。
    fn collection_transaction_paths(&self, id: Uuid) -> Result<(PathBuf, PathBuf)> {
        let root = self.root.canonicalize()?;
        let directory = self.root.join("collections");
        if directory.canonicalize()? != root.join("collections") {
            bail!("集合事务目录不能使用符号链接或 junction");
        }
        let target = directory.join(id.to_string());
        let tombstone = directory.join(format!(".collection-delete-{id}.tombstone"));
        for path in [&target, &tombstone] {
            match fs::symlink_metadata(path) {
                Ok(metadata) => {
                    let expected = root.join("collections").join(path.file_name().unwrap());
                    if metadata.file_type().is_symlink() || path.canonicalize()? != expected {
                        bail!(
                            "集合事务路径不能使用符号链接或 junction：{}",
                            path.display()
                        );
                    }
                    if !metadata.is_dir() {
                        bail!("集合事务目标不是目录：{}", path.display());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok((target, tombstone))
    }

    fn validate_collection_transaction_entry(&self, entry: &CollectionEntry) -> Result<()> {
        if Path::new(&entry.directory) != Path::new(&format!("collections/{}", entry.id)) {
            bail!("集合事务要求标准 UUID 目录：{}", entry.directory);
        }
        Ok(())
    }

    fn recover_collection_delete(
        &self,
        manifest: &WorkspaceManifest,
        id: Uuid,
        journal: &Path,
    ) -> Result<()> {
        let (target, tombstone) = self.collection_transaction_paths(id)?;
        if let Some(entry) = manifest.collections.iter().find(|entry| entry.id == id) {
            self.validate_collection_transaction_entry(entry)?;
            if target.exists() {
                if tombstone.exists() {
                    bail!("集合删除恢复发现目标与 tombstone 同时存在");
                }
            } else if tombstone.exists() {
                fs::rename(&tombstone, &target).context("恢复未提交的集合删除失败")?;
            } else {
                bail!("集合删除事务未提交但目标和 tombstone 均缺失");
            }
        } else {
            self.ensure_collection_delete_paths_unreferenced(manifest, id, &target, &tombstone)?;
            remove_directory_if_exists(&tombstone)?;
            remove_directory_if_exists(&target)?;
        }
        self.finish_transaction(journal)
    }

    fn ensure_collection_delete_paths_unreferenced(
        &self,
        manifest: &WorkspaceManifest,
        id: Uuid,
        target: &Path,
        tombstone: &Path,
    ) -> Result<()> {
        let root = self.root.canonicalize()?;
        let canonical_target = root.join("collections").join(id.to_string());
        let canonical_tombstone = root
            .join("collections")
            .join(format!(".collection-delete-{id}.tombstone"));
        // 提交前和恢复时都检查：不能删除由另一个 ID 或路径别名引用的目录。
        for entry in manifest.collections.iter().filter(|entry| entry.id != id) {
            let path = self.collection_path(entry)?;
            let canonical = match path.canonicalize() {
                Ok(path) => Some(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            if path == target
                || path == tombstone
                || canonical
                    .as_ref()
                    .is_some_and(|path| path == &canonical_target || path == &canonical_tombstone)
            {
                bail!("待删除集合目录仍被工作区清单引用");
            }
        }
        Ok(())
    }

    fn recover_collection_rename(
        &self,
        manifest: &WorkspaceManifest,
        id: Uuid,
        journal: &Path,
    ) -> Result<()> {
        let entry = manifest
            .collections
            .iter()
            .find(|entry| entry.id == id)
            .with_context(|| format!("集合重命名恢复找不到集合：{id}"))?;
        self.validate_collection_transaction_entry(entry)?;
        let (target, _) = self.collection_transaction_paths(id)?;
        let marker = target.join(COLLECTION_MARKER);
        let mut document: ContainerDocument = read_yaml(&marker)?;
        if document.id != id {
            bail!("集合重命名恢复发现文档 ID 与清单不一致");
        }
        // manifest 是唯一提交点；只恢复名称，保留鉴权和设置等其他字段。
        if document.name != entry.name {
            document.name = entry.name.clone();
            write_yaml(&marker, &document)?;
        }
        self.finish_transaction(journal)
    }

    fn rename_collection_with_manifest_writer(
        &self,
        id: Uuid,
        name: String,
        write_manifest: impl FnOnce(&Path, &WorkspaceManifest) -> Result<()>,
    ) -> Result<WorkspaceSnapshot> {
        let name = required_name(&name)?;
        let mut manifest = self.read_manifest()?;
        let entry = manifest
            .collections
            .iter_mut()
            .find(|entry| entry.id == id)
            .with_context(|| format!("工作区清单中没有集合：{id}"))?;
        self.validate_collection_transaction_entry(entry)?;
        let (target, tombstone) = self.collection_transaction_paths(id)?;
        if tombstone.exists() {
            bail!("集合重命名前存在待恢复的删除 tombstone");
        }
        let marker = target.join(COLLECTION_MARKER);
        let mut document: ContainerDocument = read_yaml(&marker)?;
        if document.id != id {
            bail!("集合文档 ID 与清单不一致");
        }
        let journal = self.begin_transaction(WorkspaceTransactionOperation::CollectionRename {
            collection_id: id,
        })?;
        entry.name = name.clone();
        document.name = name;
        let publish = write_yaml(&marker, &document)
            .and_then(|()| write_manifest(&self.manifest_path(), &manifest));
        if let Err(error) = publish {
            // 根据磁盘上的 manifest 恢复，而不猜测提交点是否经过。
            // 恢复失败保留 journal，供下次打开继续处理。
            return match self.recover_collection_rename(&self.read_manifest()?, id, &journal) {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("集合重命名失败，且恢复失败：{cleanup:#}")))
                }
            };
        }
        self.finish_transaction(&journal)
            .context("集合重命名已提交，但无法清理恢复 journal")?;
        self.snapshot()
    }

    fn delete_collection_with_manifest_writer(
        &self,
        id: Uuid,
        write_manifest: impl FnOnce(&Path, &WorkspaceManifest) -> Result<()>,
    ) -> Result<WorkspaceSnapshot> {
        let mut manifest = self.read_manifest()?;
        let entry = manifest
            .collections
            .iter()
            .find(|entry| entry.id == id)
            .with_context(|| format!("工作区清单中没有集合：{id}"))?;
        self.validate_collection_transaction_entry(entry)?;
        let (target, tombstone) = self.collection_transaction_paths(id)?;
        if !target.is_dir() || tombstone.exists() {
            bail!("集合删除目标缺失或存在待恢复的 tombstone");
        }
        self.ensure_collection_delete_paths_unreferenced(&manifest, id, &target, &tombstone)?;
        let journal = self.begin_transaction(WorkspaceTransactionOperation::CollectionDelete {
            collection_id: id,
        })?;
        manifest.collections.retain(|entry| entry.id != id);
        if let Err(error) = write_manifest(&self.manifest_path(), &manifest) {
            return match self.recover_collection_delete(&self.read_manifest()?, id, &journal) {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("集合删除失败，且恢复失败：{cleanup:#}")))
                }
            };
        }
        // 先提交 manifest，再隐藏/清理目录；后续失败保留 journal，不回滚提交。
        fs::rename(&target, &tombstone).context("集合删除已提交，但无法暂存待清理目录")?;
        self.recover_collection_delete(&manifest, id, &journal)
            .context("集合删除已提交，但无法清理目录或恢复 journal")?;
        self.snapshot()
    }

    fn rollback_environment_create(
        &self,
        journal: &Path,
        staging: &Path,
        target: &Path,
        published: bool,
    ) -> Result<()> {
        if published {
            remove_file_if_exists(target)?;
        }
        remove_file_if_exists(staging)?;
        self.finish_transaction(journal)
    }

    fn rollback_environment_delete(&self, journal: &Path) -> Result<()> {
        self.finish_transaction(journal)
    }

    /// Bind history and other session data to the application-owned runtime
    /// directory. Existing history from older workspace layouts is moved once.
    pub fn with_runtime_root(mut self, runtime_root: impl Into<PathBuf>) -> Result<Self> {
        let runtime_root = runtime_root.into();
        let target = runtime_root.join(HISTORY_DIR);
        fs::create_dir_all(&runtime_root)?;
        let legacy = self.root.join(HISTORY_DIR);
        if legacy.is_dir() && !target.exists() {
            fs::rename(&legacy, &target).or_else(|_| {
                fs::create_dir_all(&target)?;
                for entry in fs::read_dir(&legacy)? {
                    let entry = entry?;
                    fs::rename(entry.path(), target.join(entry.file_name()))?;
                }
                fs::remove_dir(&legacy)
            })?;
        } else {
            fs::create_dir_all(&target)?;
        }
        self.runtime_root = Some(runtime_root);
        Ok(self)
    }

    fn history_directory(&self) -> PathBuf {
        self.runtime_root
            .as_ref()
            .map(|root| root.join(HISTORY_DIR))
            .unwrap_or_else(|| self.root.join(HISTORY_DIR))
    }

    pub fn id(&self) -> Result<Uuid> {
        Ok(self.read_manifest()?.id)
    }

    pub fn name(&self) -> Result<String> {
        Ok(self.read_manifest()?.name)
    }

    pub fn snapshot(&self) -> Result<WorkspaceSnapshot> {
        let manifest = self.read_manifest()?;
        let environments = self.read_environments(&manifest)?;
        let mut located = Vec::new();

        for entry in &manifest.collections {
            let path = self.collection_path(entry)?;
            let document: ContainerDocument = read_yaml(&path.join(COLLECTION_MARKER))
                .with_context(|| format!("读取集合文件失败：{}", path.display()))?;
            if document.id != entry.id {
                bail!("集合目录 {} 的 id 与工作区清单不一致", path.display());
            }

            located.push(LocatedNode {
                node: WorkspaceNode {
                    id: entry.id,
                    parent_id: None,
                    depth: 0,
                    name: document.name,
                    method: None,
                    is_folder: true,
                    expanded: true,
                    kind: WorkspaceNodeKind::Collection,
                    debug_case_count: 0,
                },
                path: path.clone(),
            });
            Self::scan_children(&path, Some(entry.id), 1, &mut located)?;
        }

        Ok(WorkspaceSnapshot {
            id: manifest.id,
            name: manifest.name,
            nodes: located.into_iter().map(|item| item.node).collect(),
            environments,
        })
    }

    pub fn record_history(
        &self,
        created_at_ms: u64,
        spec: PublicRequestPreview,
        response: PublicResponseSummary,
        body_source: &Path,
        debug_case_id: Option<Uuid>,
        environment_id: Option<Uuid>,
    ) -> Result<()> {
        // Defense in depth: IPC input must already be public, but callers can still construct a
        // DTO manually or deserialize a legacy-shaped payload. Apply the raw preview's source
        // annotations to the response before masking that preview itself.
        let response = spec.redact_response_summary(response);
        let spec = spec.redacted();
        let id = response.id;
        let duration_ms = response.timing.total_ms;
        let directory = self.history_directory();
        fs::create_dir_all(&directory)?;
        let body_target = directory.join(format!("{id}.body"));
        let body_temp = directory.join(format!(".{id}.body.tmp"));
        fs::copy(body_source, &body_temp).with_context(|| "复制历史响应体失败")?;
        if body_target.exists() {
            fs::remove_file(&body_target)?;
        }
        fs::rename(body_temp, &body_target)?;
        write_yaml(
            &directory.join(format!("{id}.yaml")),
            &HistoryRecord {
                id,
                created_at_ms,
                debug_case_id,
                environment_id,
                spec,
                response: Some(response),
                error: None,
                actual_request: None,
                duration_ms,
            },
        )?;
        self.trim_history()
    }

    /// 记录未收到 HTTP 响应的运行；只持久化净化后的错误和最终请求，不创建 body 文件。
    pub fn record_failed_history(&self, input: FailedHistoryWriteInput) -> Result<()> {
        let FailedHistoryWriteInput {
            created_at_ms,
            spec,
            error,
            actual_request,
            duration_ms,
            debug_case_id,
            environment_id,
        } = input;
        // Keep legacy-shaped public input from bypassing a `Secret` / `File` row whose field name
        // is neutral. The preview is still raw at this point and is only used to mask the actual
        // snapshot before either value is persisted.
        let actual_request = actual_request.map(|request| spec.redact_actual_request(request));
        let id = Uuid::new_v4();
        let directory = self.history_directory();
        fs::create_dir_all(&directory)?;
        write_yaml(
            &directory.join(format!("{id}.yaml")),
            &HistoryRecord {
                id,
                created_at_ms,
                debug_case_id,
                environment_id,
                spec: spec.redacted(),
                response: None,
                error: Some(error.redacted()),
                actual_request,
                duration_ms,
            },
        )?;
        self.trim_history()
    }

    pub fn list_history(&self) -> Result<Vec<HistoryEntry>> {
        let mut records = self.read_history_records()?;
        records.sort_by_key(|record| std::cmp::Reverse(record.created_at_ms));
        Ok(records
            .into_iter()
            .map(|record| {
                let status = record
                    .response
                    .as_ref()
                    .and_then(|response| response.status);
                let size_bytes = record
                    .response
                    .as_ref()
                    .map(|response| response.size_bytes)
                    .unwrap_or_default();
                let duration_ms = record
                    .response
                    .as_ref()
                    .map(|response| response.timing.total_ms)
                    .unwrap_or(record.duration_ms);
                HistoryEntry {
                    id: record.id,
                    created_at_ms: record.created_at_ms,
                    debug_case_id: record.debug_case_id,
                    environment_id: record.environment_id,
                    method: record.spec.method,
                    url: record.spec.url,
                    status,
                    size_bytes,
                    duration_ms,
                }
            })
            .collect())
    }

    /// 成功记录带响应体路径；失败记录没有响应体，因此返回 `None`。
    pub fn read_history(&self, id: Uuid) -> Result<(HistoryRecord, Option<PathBuf>)> {
        let directory = self.history_directory();
        let record: HistoryRecord = read_yaml(&directory.join(format!("{id}.yaml")))?;
        let record = record.redacted();
        if record.id != id {
            bail!("历史记录 id 不一致：{id}");
        }
        let body = if record.response.is_some() {
            let body = directory.join(format!("{id}.body"));
            if !body.exists() {
                bail!("历史响应体不存在：{id}");
            }
            Some(body)
        } else {
            None
        };
        Ok((record, body))
    }

    pub fn clear_history(&self) -> Result<()> {
        let directory = self.history_directory();
        if directory.exists() {
            fs::remove_dir_all(&directory)?;
        }
        fs::create_dir_all(directory)?;
        Ok(())
    }

    fn read_history_records(&self) -> Result<Vec<HistoryRecord>> {
        let directory = self.history_directory();
        let mut records = Vec::new();
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "yaml")
            {
                let record: HistoryRecord = read_yaml(&path)?;
                records.push(record.redacted());
            }
        }
        Ok(records)
    }

    fn trim_history(&self) -> Result<()> {
        let mut records = self.read_history_records()?;
        records.sort_by_key(|record| std::cmp::Reverse(record.created_at_ms));
        for record in records.into_iter().skip(HISTORY_LIMIT) {
            let directory = self.history_directory();
            for extension in ["yaml", "body"] {
                let path = directory.join(format!("{}.{}", record.id, extension));
                if path.exists() {
                    fs::remove_file(path)?;
                }
            }
        }
        Ok(())
    }

    pub fn read_request(&self, id: Uuid) -> Result<WorkspaceRequest> {
        let location = self.locate(id)?;
        if !matches!(location.node.kind, WorkspaceNodeKind::Request) {
            bail!("节点不是请求：{id}");
        }
        let document: PersistedRequest = read_yaml(&location.path)
            .with_context(|| format!("读取请求文件失败：{}", location.path.display()))?;
        if document.id != id {
            bail!("请求文件 {} 的 id 不一致", location.path.display());
        }
        Ok(document.as_ipc())
    }

    pub fn read_container(&self, id: Uuid) -> Result<WorkspaceContainer> {
        let location = self.locate(id)?;
        let marker = container_marker(&location.node.kind)?;
        let document: ContainerDocument = read_yaml(&location.path.join(marker))?;
        Ok(WorkspaceContainer {
            id: document.id,
            name: document.name,
            auth: document.auth,
            settings: document.settings,
        })
    }

    pub fn save_container(&self, container: WorkspaceContainer) -> Result<WorkspaceContainer> {
        let location = self.locate(container.id)?;
        let marker = container_marker(&location.node.kind)?;
        let name = required_name(&container.name)?;
        let document = ContainerDocument {
            apogee: WORKSPACE_FORMAT_VERSION,
            id: container.id,
            name: name.clone(),
            auth: container.auth,
            settings: container.settings,
        };
        write_yaml(&location.path.join(marker), &document)?;
        if matches!(location.node.kind, WorkspaceNodeKind::Collection) {
            let mut manifest = self.read_manifest()?;
            let entry = manifest
                .collections
                .iter_mut()
                .find(|entry| entry.id == container.id)
                .with_context(|| format!("工作区清单中没有集合：{}", container.id))?;
            entry.name = name;
            write_yaml(&self.manifest_path(), &manifest)?;
        }
        Ok(WorkspaceContainer {
            id: document.id,
            name: document.name,
            auth: document.auth,
            settings: document.settings,
        })
    }

    pub fn resolve_request_inheritance(
        &self,
        request_id: Uuid,
        spec: RequestSpec,
    ) -> Result<RequestSpec> {
        self.resolve_inheritance(request_id, spec, false)
    }

    /// Token operations inherit only network settings; auth is explicitly supplied by the editor.
    /// For a container, its edited current settings override ancestors just like a request.
    pub fn resolve_token_request_inheritance(
        &self,
        node_id: Uuid,
        spec: RequestSpec,
    ) -> Result<RequestSpec> {
        self.resolve_inheritance(node_id, spec, true)
    }

    /// Sensitive Auth provenance; never put this projection in history/response events.
    pub(crate) fn resolve_auth_source(
        &self,
        node_id: Uuid,
        auth: AuthConfig,
    ) -> Result<(AuthConfig, Option<Uuid>)> {
        let spec = RequestSpec {
            method: "POST".into(),
            url: String::new(),
            params: vec![],
            path_params: vec![],
            headers: vec![],
            body: Body::None,
            auth,
            settings: RequestSettings::default(),
        };
        let (spec, source) = self.resolve_inheritance_with_source(node_id, spec, true)?;
        Ok((spec.auth, source))
    }

    fn resolve_inheritance(
        &self,
        request_id: Uuid,
        spec: RequestSpec,
        allow_container: bool,
    ) -> Result<RequestSpec> {
        self.resolve_inheritance_with_source(request_id, spec, allow_container)
            .map(|(spec, _)| spec)
    }

    fn resolve_inheritance_with_source(
        &self,
        request_id: Uuid,
        mut spec: RequestSpec,
        allow_container: bool,
    ) -> Result<(RequestSpec, Option<Uuid>)> {
        let locations = self.scan()?;
        let mut current = locations
            .iter()
            .find(|item| item.node.id == request_id)
            .with_context(|| format!("未知的工作区节点：{request_id}"))?;
        if !(matches!(current.node.kind, WorkspaceNodeKind::Request)
            || allow_container
                && matches!(
                    current.node.kind,
                    WorkspaceNodeKind::Collection | WorkspaceNodeKind::Folder
                ))
        {
            bail!("节点不是请求：{request_id}");
        }
        let mut ancestors = Vec::new();
        while let Some(parent_id) = current.node.parent_id {
            current = locations
                .iter()
                .find(|item| item.node.id == parent_id)
                .with_context(|| format!("请求 {request_id} 的上级节点不存在：{parent_id}"))?;
            ancestors.push(current);
        }
        ancestors.reverse();

        let mut auth = AuthConfig::Inherit;
        let mut source = None;
        let mut settings = RequestSettings::default();
        for ancestor in ancestors {
            let marker = container_marker(&ancestor.node.kind)?;
            let container: ContainerDocument = read_yaml(&ancestor.path.join(marker))?;
            if !matches!(container.auth, AuthConfig::Inherit) {
                auth = container.auth;
                source = Some(ancestor.node.id);
            }
            merge_settings(&mut settings, &container.settings);
        }
        if matches!(spec.auth, AuthConfig::Inherit) {
            spec.auth = auth;
        } else {
            source = Some(request_id);
        }
        merge_settings(&mut settings, &spec.settings);
        spec.settings = settings;
        Ok((spec, source))
    }

    pub(crate) fn read_auth(&self, id: Uuid) -> Result<AuthConfig> {
        let location = self.locate(id)?;
        if matches!(location.node.kind, WorkspaceNodeKind::Request) {
            return Ok(self.read_request(id)?.spec.auth);
        }
        let marker = container_marker(&location.node.kind)?;
        let document: ContainerDocument = read_yaml(&location.path.join(marker))?;
        if document.id != id {
            bail!("OAUTH2_SOURCE_STALE");
        }
        Ok(document.auth)
    }

    pub(crate) fn ensure_auth_unchanged(&self, id: Uuid, expected: &AuthConfig) -> Result<()> {
        if serde_json::to_value(self.read_auth(id)?)? != serde_json::to_value(expected)? {
            bail!("AUTH_CONFIG_STALE");
        }
        Ok(())
    }

    /// Only Auth changes. Preserve current names/settings/body/cases and never rewrite a manifest.
    pub(crate) fn compare_and_set_auth(
        &self,
        id: Uuid,
        expected: &AuthConfig,
        updated: AuthConfig,
    ) -> Result<()> {
        let location = self.locate(id)?;
        if matches!(location.node.kind, WorkspaceNodeKind::Request) {
            let mut document: PersistedRequest = read_yaml(&location.path)?;
            if document.id != id
                || serde_json::to_value(&document.spec.auth)? != serde_json::to_value(expected)?
            {
                bail!("OAUTH2_SOURCE_STALE");
            }
            document.spec.auth = updated;
            write_yaml(&location.path, &document)
        } else {
            let path = location.path.join(container_marker(&location.node.kind)?);
            let mut document: ContainerDocument = read_yaml(&path)?;
            if document.id != id
                || serde_json::to_value(&document.auth)? != serde_json::to_value(expected)?
            {
                bail!("OAUTH2_SOURCE_STALE");
            }
            document.auth = updated;
            write_yaml(&path, &document)
        }
    }

    pub fn read_environment(&self, id: Uuid) -> Result<Environment> {
        self.read_environment_after_manifest(id, || Ok(()))
    }

    /// 为跨文件读取的并发回归测试保留的暂停边界。生产读取没有额外回调。
    #[cfg(test)]
    pub(crate) fn read_environment_with_after_manifest(
        &self,
        id: Uuid,
        after_manifest: impl FnOnce() -> Result<()>,
    ) -> Result<Environment> {
        self.read_environment_after_manifest(id, after_manifest)
    }

    fn read_environment_after_manifest(
        &self,
        id: Uuid,
        after_manifest: impl FnOnce() -> Result<()>,
    ) -> Result<Environment> {
        let manifest = self.read_manifest()?;
        if !manifest.environments.iter().any(|entry| entry.id == id) {
            bail!("未知的环境：{id}");
        }
        after_manifest()?;
        let environment: Environment =
            read_yaml(&self.environment_path(id)).with_context(|| {
                format!("读取环境文件失败：{}", self.environment_path(id).display())
            })?;
        if environment.id != id {
            bail!(
                "环境文件 {} 的 id 不一致",
                self.environment_path(id).display()
            );
        }
        Ok(environment)
    }

    pub fn create_environment(&self, name: String) -> Result<Environment> {
        self.create_environment_with_manifest_writer(name, |path, manifest| {
            write_yaml(path, manifest)
        })
    }

    /// 以 journal → staging → publish → manifest 的顺序创建环境。测试可注入 manifest
    /// 发布失败，验证失败时不会留下已发布但未引用的 environment 文件。
    pub(crate) fn create_environment_with_manifest_writer(
        &self,
        name: String,
        write_manifest: impl FnOnce(&Path, &WorkspaceManifest) -> Result<()>,
    ) -> Result<Environment> {
        let name = required_name(&name)?;
        let environment = Environment {
            id: Uuid::new_v4(),
            name: name.clone(),
            host: String::new(),
            variables: Vec::new(),
        };
        let journal = self.begin_transaction(WorkspaceTransactionOperation::EnvironmentCreate {
            environment_id: environment.id,
        })?;
        let staging = self.environment_create_staging_path(environment.id);
        let target = self.environment_path(environment.id);
        let mut published = false;

        if let Err(error) = write_yaml(&staging, &environment) {
            let rollback = self.rollback_environment_create(&journal, &staging, &target, published);
            return match rollback {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.context(format!(
                    "环境创建写入 staging 失败，且回滚失败：{cleanup:#}"
                ))),
            };
        }
        let staged = read_yaml::<Environment>(&staging)
            .with_context(|| format!("校验环境 staging 失败：{}", staging.display()));
        match staged {
            Ok(staged) if staged.id == environment.id => {}
            Ok(_) => {
                let error = anyhow::anyhow!("环境 staging 的 id 与事务不一致");
                let rollback =
                    self.rollback_environment_create(&journal, &staging, &target, published);
                return match rollback {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(error.context(format!(
                        "环境创建 staging 校验失败，且回滚失败：{cleanup:#}"
                    ))),
                };
            }
            Err(error) => {
                let rollback =
                    self.rollback_environment_create(&journal, &staging, &target, published);
                return match rollback {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(error.context(format!(
                        "环境创建 staging 校验失败，且回滚失败：{cleanup:#}"
                    ))),
                };
            }
        }

        if target.exists() {
            let error = anyhow::anyhow!("环境创建目标已存在：{}", target.display());
            let rollback = self.rollback_environment_create(&journal, &staging, &target, published);
            return match rollback {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.context(format!(
                    "环境创建不能发布到已有目标，且回滚失败：{cleanup:#}"
                ))),
            };
        }
        if let Err(error) = fs::rename(&staging, &target)
            .with_context(|| format!("发布环境 staging 失败：{}", target.display()))
        {
            let rollback = self.rollback_environment_create(&journal, &staging, &target, published);
            return match rollback {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("环境创建发布失败，且回滚失败：{cleanup:#}")))
                }
            };
        }
        published = true;

        let mut manifest = match self.read_manifest() {
            Ok(manifest) => manifest,
            Err(error) => {
                let rollback =
                    self.rollback_environment_create(&journal, &staging, &target, published);
                return match rollback {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(error.context(format!(
                        "环境创建读取 manifest 失败，且回滚失败：{cleanup:#}"
                    ))),
                };
            }
        };
        if manifest
            .environments
            .iter()
            .any(|entry| entry.id == environment.id)
        {
            let error = anyhow::anyhow!("环境创建 UUID 已存在于 manifest：{}", environment.id);
            let rollback = self.rollback_environment_create(&journal, &staging, &target, published);
            return match rollback {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("环境创建 manifest 冲突，且回滚失败：{cleanup:#}")))
                }
            };
        }
        manifest.environments.push(EnvironmentEntry {
            id: environment.id,
            name,
        });
        if let Err(error) = write_manifest(&self.manifest_path(), &manifest) {
            let rollback = self.rollback_environment_create(&journal, &staging, &target, published);
            return match rollback {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.context(format!(
                    "环境创建 manifest 发布失败，且回滚失败：{cleanup:#}"
                ))),
            };
        }
        if let Err(error) = self.finish_transaction(&journal) {
            return Err(error.context("环境创建已提交，但无法清理恢复 journal"));
        }

        Ok(environment)
    }

    pub fn save_environment(&self, mut environment: Environment) -> Result<Environment> {
        environment.name = required_name(&environment.name)?;
        environment.host = normalize_environment_host(&environment.host)?;
        let mut manifest = self.read_manifest()?;
        let entry = manifest
            .environments
            .iter_mut()
            .find(|entry| entry.id == environment.id)
            .with_context(|| format!("未知的环境：{}", environment.id))?;
        entry.name = environment.name.clone();
        write_yaml(&self.environment_path(environment.id), &environment)?;
        write_yaml(&self.manifest_path(), &manifest)?;
        Ok(environment)
    }

    pub fn delete_environment(&self, id: Uuid) -> Result<WorkspaceSnapshot> {
        self.delete_environment_with_manifest_writer(id, |path, manifest| {
            write_yaml(path, manifest)
        })
    }

    /// 以 journal → manifest → tombstone cleanup 的顺序删除环境。manifest 是提交点：
    /// 在它发布前，读取者始终能找到旧环境文件；发布后，读取者会忽略尚待清理的
    /// 文件。测试可注入 manifest 发布失败，验证未提交删除不会改变可见环境。
    pub(crate) fn delete_environment_with_manifest_writer(
        &self,
        id: Uuid,
        write_manifest: impl FnOnce(&Path, &WorkspaceManifest) -> Result<()>,
    ) -> Result<WorkspaceSnapshot> {
        let mut manifest = self.read_manifest()?;
        let original_len = manifest.environments.len();
        manifest.environments.retain(|entry| entry.id != id);
        if manifest.environments.len() == original_len {
            bail!("未知的环境：{id}");
        }

        let journal = self.begin_transaction(WorkspaceTransactionOperation::EnvironmentDelete {
            environment_id: id,
        })?;
        let target = self.environment_path(id);
        let tombstone = self.environment_delete_tombstone_path(id);
        if tombstone.exists() {
            let error = anyhow::anyhow!("环境删除 tombstone 已存在：{}", tombstone.display());
            let rollback = self.rollback_environment_delete(&journal);
            return match rollback {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("环境删除无法开始，且回滚失败：{cleanup:#}")))
                }
            };
        }
        if let Err(error) = write_manifest(&self.manifest_path(), &manifest) {
            let rollback = self.rollback_environment_delete(&journal);
            return match rollback {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.context(format!(
                    "环境删除 manifest 发布失败，且回滚失败：{cleanup:#}"
                ))),
            };
        }

        // manifest 发布后删除已提交：即使文件清理失败也不能回滚 manifest，而是保留
        // journal 让下次打开根据已提交 manifest 完成 target/tombstone 清理。
        if target.exists() {
            if let Err(error) = fs::rename(&target, &tombstone)
                .with_context(|| format!("暂存已提交的待删除环境失败：{}", target.display()))
            {
                return Err(error.context("环境删除已提交，但无法暂存待清理环境"));
            }
        }
        if let Err(error) = remove_file_if_exists(&tombstone) {
            return Err(error.context("环境删除已提交，但无法清理 tombstone"));
        }
        if let Err(error) = self.finish_transaction(&journal) {
            return Err(error.context("环境删除已提交，但无法清理恢复 journal"));
        }
        self.snapshot()
    }

    pub fn list_debug_cases(&self, request_id: Uuid) -> Result<Vec<DebugCase>> {
        let location = self.locate(request_id)?;
        if !matches!(location.node.kind, WorkspaceNodeKind::Request) {
            bail!("节点不是请求：{request_id}");
        }
        let document: PersistedRequest = read_yaml(&location.path)
            .with_context(|| format!("读取请求文件失败：{}", location.path.display()))?;
        if document.id != request_id {
            bail!("请求文件 {} 的 id 不一致", location.path.display());
        }
        Ok(document.debug_cases)
    }

    pub fn create_debug_case(
        &self,
        request_id: Uuid,
        name: String,
        case_type: apogee_core::workspace::DebugCaseType,
        spec: RequestSpec,
    ) -> Result<DebugCase> {
        let name = required_name(&name)?;
        let location = self.locate(request_id)?;
        if !matches!(location.node.kind, WorkspaceNodeKind::Request) {
            bail!("节点不是请求：{request_id}");
        }
        let mut document: PersistedRequest = read_yaml(&location.path)
            .with_context(|| format!("读取请求文件失败：{}", location.path.display()))?;
        if document.id != request_id {
            bail!("请求文件 {} 的 id 不一致", location.path.display());
        }
        let now = epoch_ms();
        let debug_case = DebugCase {
            id: Uuid::new_v4(),
            request_id,
            name,
            case_type,
            spec,
            created_at_ms: now,
            updated_at_ms: now,
        };
        document.debug_cases.push(debug_case.clone());
        write_yaml(&location.path, &document)?;
        Ok(debug_case)
    }

    pub fn delete_debug_case(&self, request_id: Uuid, case_id: Uuid) -> Result<()> {
        let location = self.locate(request_id)?;
        if !matches!(location.node.kind, WorkspaceNodeKind::Request) {
            bail!("节点不是请求：{request_id}");
        }
        let mut document: PersistedRequest = read_yaml(&location.path)
            .with_context(|| format!("读取请求文件失败：{}", location.path.display()))?;
        if document.id != request_id {
            bail!("请求文件 {} 的 id 不一致", location.path.display());
        }
        let original_len = document.debug_cases.len();
        document
            .debug_cases
            .retain(|debug_case| debug_case.id != case_id);
        if document.debug_cases.len() == original_len {
            bail!("未知的调试用例：{case_id}");
        }
        write_yaml(&location.path, &document)
    }

    pub fn save_request(&self, id: Uuid, name: String, spec: RequestSpec) -> Result<()> {
        let name = required_name(&name)?;
        let location = self.locate(id)?;
        if !matches!(location.node.kind, WorkspaceNodeKind::Request) {
            bail!("节点不是请求：{id}");
        }
        let document: PersistedRequest = read_yaml(&location.path)
            .with_context(|| format!("读取请求文件失败：{}", location.path.display()))?;
        if document.id != id {
            bail!("请求文件 {} 的 id 不一致", location.path.display());
        }
        write_yaml(
            &location.path,
            &PersistedRequest {
                apogee: WORKSPACE_FORMAT_VERSION,
                id,
                name,
                spec,
                debug_cases: document.debug_cases,
            },
        )
    }

    pub fn create_collection(&self, name: String) -> Result<WorkspaceSnapshot> {
        let name = required_name(&name)?;
        let id = Uuid::new_v4();
        let directory = format!("collections/{id}");
        let path = self.root.join(&directory);
        fs::create_dir_all(&path)?;
        write_yaml(
            &path.join(COLLECTION_MARKER),
            &ContainerDocument {
                apogee: WORKSPACE_FORMAT_VERSION,
                id,
                name: name.clone(),
                auth: AuthConfig::Inherit,
                settings: RequestSettings::default(),
            },
        )?;

        let mut manifest = self.read_manifest()?;
        manifest.collections.push(CollectionEntry {
            id,
            name,
            directory,
        });
        write_yaml(&self.manifest_path(), &manifest)?;
        self.snapshot()
    }

    /// 将完整 YApi 导入计划先写入暂存目录，校验后一次发布集合与环境。
    /// 发布清单失败时会移除已发布文件并恢复旧清单，不留下可见的半成品。
    pub fn import_yapi_plan(
        &self,
        plan: &apogee_core::yapi::YapiImportPlan,
        requested_name: Option<&str>,
        import_environments: bool,
    ) -> Result<YapiWorkspaceImportResult> {
        use std::collections::HashMap;

        if plan.requests.is_empty() {
            bail!("YApi 项目中没有可导入的请求");
        }
        let mut manifest = self.read_manifest()?;
        let base_name = requested_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&plan.project_name);
        let collection_name = unique_name(
            base_name,
            manifest.collections.iter().map(|entry| entry.name.as_str()),
        );
        let collection_id = Uuid::new_v4();
        let directory = format!("collections/{collection_id}");
        let final_collection = self.root.join(&directory);
        let staging_collection = self
            .root
            .join("collections")
            .join(format!(".{collection_id}.yapi-import"));
        let staging_environments = self
            .root
            .join(ENVIRONMENT_DIR)
            .join(format!(".{collection_id}.yapi-import"));

        fs::create_dir_all(&staging_collection)?;
        let prepare = (|| -> Result<(usize, Vec<Environment>)> {
            write_yaml(
                &staging_collection.join(COLLECTION_MARKER),
                &ContainerDocument {
                    apogee: WORKSPACE_FORMAT_VERSION,
                    id: collection_id,
                    name: collection_name.clone(),
                    auth: AuthConfig::Inherit,
                    settings: RequestSettings::default(),
                },
            )?;

            let mut folders = HashMap::<String, (Uuid, PathBuf)>::new();
            for item in &plan.requests {
                let mut parent = staging_collection.clone();
                let mut path_key = String::new();
                for folder_name in &item.folder {
                    let folder_name = required_name(folder_name)?;
                    if !path_key.is_empty() {
                        path_key.push('/');
                    }
                    path_key.push_str(&folder_name);
                    if let Some((_, path)) = folders.get(&path_key) {
                        parent = path.clone();
                        continue;
                    }
                    let id = Uuid::new_v4();
                    let path = parent.join(id.to_string());
                    fs::create_dir_all(&path)?;
                    write_yaml(
                        &path.join(FOLDER_MARKER),
                        &ContainerDocument {
                            apogee: WORKSPACE_FORMAT_VERSION,
                            id,
                            name: folder_name,
                            auth: AuthConfig::Inherit,
                            settings: RequestSettings::default(),
                        },
                    )?;
                    folders.insert(path_key.clone(), (id, path.clone()));
                    parent = path;
                }
                let request_id = Uuid::new_v4();
                write_yaml(
                    &parent.join(format!("{request_id}.yaml")),
                    &PersistedRequest {
                        apogee: WORKSPACE_FORMAT_VERSION,
                        id: request_id,
                        name: required_name(&item.name)?,
                        spec: item.spec.clone(),
                        debug_cases: Vec::new(),
                    },
                )?;
            }

            // 重新读取全部暂存 YAML，防止序列化或磁盘错误进入正式工作区。
            validate_import_tree(&staging_collection)?;

            let mut environments = Vec::new();
            if import_environments {
                fs::create_dir_all(&staging_environments)?;
                let existing_names: Vec<&str> = manifest
                    .environments
                    .iter()
                    .map(|entry| entry.name.as_str())
                    .collect();
                let mut allocated_names: Vec<String> = Vec::new();
                for source in &plan.environments {
                    let name = unique_name(
                        &source.name,
                        existing_names
                            .iter()
                            .copied()
                            .chain(allocated_names.iter().map(String::as_str)),
                    );
                    allocated_names.push(name.clone());
                    let mut environment = source.clone();
                    environment.id = Uuid::new_v4();
                    environment.name = name;
                    write_yaml(
                        &staging_environments.join(format!("{}.yaml", environment.id)),
                        &environment,
                    )?;
                    let _: Environment =
                        read_yaml(&staging_environments.join(format!("{}.yaml", environment.id)))?;
                    environments.push(environment);
                }
            }
            Ok((folders.len(), environments))
        })();

        let (imported_folders, environments) = match prepare {
            Ok(value) => value,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging_collection);
                let _ = fs::remove_dir_all(&staging_environments);
                return Err(error);
            }
        };

        let old_manifest = manifest.clone();
        let mut published_environments = Vec::new();
        let publish = (|| -> Result<()> {
            fs::rename(&staging_collection, &final_collection)
                .with_context(|| "发布 YApi 集合失败")?;
            for environment in &environments {
                let staged = staging_environments.join(format!("{}.yaml", environment.id));
                let target = self.environment_path(environment.id);
                fs::rename(&staged, &target).with_context(|| "发布 YApi 环境失败")?;
                published_environments.push(target);
            }
            manifest.collections.push(CollectionEntry {
                id: collection_id,
                name: collection_name.clone(),
                directory,
            });
            manifest
                .environments
                .extend(environments.iter().map(|environment| EnvironmentEntry {
                    id: environment.id,
                    name: environment.name.clone(),
                }));
            write_yaml(&self.manifest_path(), &manifest)?;
            // 只有完整快照可重读时才视为提交成功。
            self.snapshot()?;
            Ok(())
        })();

        let _ = fs::remove_dir_all(&staging_environments);
        if let Err(error) = publish {
            let _ = fs::remove_dir_all(&final_collection);
            for path in published_environments {
                let _ = fs::remove_file(path);
            }
            let _ = write_yaml(&self.manifest_path(), &old_manifest);
            return Err(error);
        }

        Ok(YapiWorkspaceImportResult {
            collection_id,
            collection_name,
            imported_requests: plan.requests.len(),
            imported_folders,
            imported_environments: environments.len(),
            warnings: plan.warnings.clone(),
        })
    }

    pub fn create_folder(&self, parent_id: Uuid, name: String) -> Result<WorkspaceSnapshot> {
        let name = required_name(&name)?;
        let parent = self.locate(parent_id)?;
        if !matches!(
            parent.node.kind,
            WorkspaceNodeKind::Collection | WorkspaceNodeKind::Folder
        ) {
            bail!("文件夹必须创建在集合或文件夹下：{parent_id}");
        }

        let id = Uuid::new_v4();
        let path = parent.path.join(id.to_string());
        fs::create_dir_all(&path)?;
        write_yaml(
            &path.join(FOLDER_MARKER),
            &ContainerDocument {
                apogee: WORKSPACE_FORMAT_VERSION,
                id,
                name,
                auth: AuthConfig::Inherit,
                settings: RequestSettings::default(),
            },
        )?;
        self.snapshot()
    }

    pub fn create_request(&self, parent_id: Uuid, name: String) -> Result<WorkspaceRequest> {
        let name = required_name(&name)?;
        let parent = self.locate(parent_id)?;
        if !matches!(
            parent.node.kind,
            WorkspaceNodeKind::Collection | WorkspaceNodeKind::Folder
        ) {
            bail!("请求必须创建在集合或文件夹下：{parent_id}");
        }

        let id = Uuid::new_v4();
        let document = PersistedRequest {
            apogee: WORKSPACE_FORMAT_VERSION,
            id,
            name,
            spec: default_request_spec(),
            debug_cases: Vec::new(),
        };
        let path = parent.path.join(format!("{id}.yaml"));
        write_yaml(&path, &document)?;
        Ok(document.as_ipc())
    }

    pub fn rename_node(&self, id: Uuid, name: String) -> Result<WorkspaceSnapshot> {
        self.recover_pending_transactions()?;
        let name = required_name(&name)?;
        let location = self.locate(id)?;
        match location.node.kind {
            WorkspaceNodeKind::Collection => {
                return self.rename_collection_with_manifest_writer(id, name, |path, manifest| {
                    write_yaml(path, manifest)
                });
            }
            WorkspaceNodeKind::Folder => {
                let mut document: ContainerDocument =
                    read_yaml(&location.path.join(FOLDER_MARKER))?;
                document.name = name;
                write_yaml(&location.path.join(FOLDER_MARKER), &document)?;
            }
            WorkspaceNodeKind::Request => {
                let mut document: PersistedRequest = read_yaml(&location.path)?;
                document.name = name;
                write_yaml(&location.path, &document)?;
            }
        }
        self.snapshot()
    }

    pub fn delete_node(&self, id: Uuid) -> Result<WorkspaceSnapshot> {
        self.recover_pending_transactions()?;
        let location = self.locate(id)?;
        match location.node.kind {
            WorkspaceNodeKind::Collection => {
                return self.delete_collection_with_manifest_writer(id, |path, manifest| {
                    write_yaml(path, manifest)
                });
            }
            WorkspaceNodeKind::Folder => {
                fs::remove_dir_all(&location.path)
                    .with_context(|| format!("删除文件夹目录失败：{}", location.path.display()))?;
            }
            WorkspaceNodeKind::Request => {
                fs::remove_file(&location.path)
                    .with_context(|| format!("删除请求文件失败：{}", location.path.display()))?;
            }
        }
        self.snapshot()
    }

    fn create_default_workspace(&self, name: String) -> Result<()> {
        let name = required_name(&name)?;
        let workspace_id = Uuid::new_v4();
        let collection_id = Uuid::new_v4();
        let environment_id = Uuid::new_v4();
        let directory = format!("collections/{collection_id}");
        let collection_path = self.root.join(&directory);
        fs::create_dir_all(&collection_path)?;
        write_yaml(
            &collection_path.join(COLLECTION_MARKER),
            &ContainerDocument {
                apogee: WORKSPACE_FORMAT_VERSION,
                id: collection_id,
                name: "我的集合".to_string(),
                auth: AuthConfig::Inherit,
                settings: RequestSettings::default(),
            },
        )?;
        write_yaml(
            &self.environment_path(environment_id),
            &Environment {
                id: environment_id,
                name: "本地".to_string(),
                host: String::new(),
                variables: Vec::new(),
            },
        )?;
        write_yaml(
            &self.manifest_path(),
            &WorkspaceManifest {
                apogee: WORKSPACE_FORMAT_VERSION,
                id: workspace_id,
                name,
                collections: vec![CollectionEntry {
                    id: collection_id,
                    name: "我的集合".to_string(),
                    directory,
                }],
                environments: vec![EnvironmentEntry {
                    id: environment_id,
                    name: "本地".to_string(),
                }],
            },
        )
    }

    fn scan(&self) -> Result<Vec<LocatedNode>> {
        let manifest = self.read_manifest()?;
        let mut nodes = Vec::new();
        for entry in &manifest.collections {
            let path = self.collection_path(entry)?;
            let document: ContainerDocument = read_yaml(&path.join(COLLECTION_MARKER))?;
            nodes.push(LocatedNode {
                node: WorkspaceNode {
                    id: entry.id,
                    parent_id: None,
                    depth: 0,
                    name: document.name,
                    method: None,
                    is_folder: true,
                    expanded: true,
                    kind: WorkspaceNodeKind::Collection,
                    debug_case_count: 0,
                },
                path: path.clone(),
            });
            Self::scan_children(&path, Some(entry.id), 1, &mut nodes)?;
        }
        Ok(nodes)
    }

    fn scan_children(
        directory: &Path,
        parent_id: Option<Uuid>,
        depth: usize,
        nodes: &mut Vec<LocatedNode>,
    ) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(directory)
            .with_context(|| format!("读取集合目录失败：{}", directory.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                let marker = path.join(FOLDER_MARKER);
                if !marker.exists() {
                    continue;
                }
                let document: ContainerDocument = read_yaml(&marker)?;
                nodes.push(LocatedNode {
                    node: WorkspaceNode {
                        id: document.id,
                        parent_id,
                        depth,
                        name: document.name,
                        method: None,
                        is_folder: true,
                        expanded: false,
                        kind: WorkspaceNodeKind::Folder,
                        debug_case_count: 0,
                    },
                    path: path.clone(),
                });
                Self::scan_children(&path, Some(document.id), depth + 1, nodes)?;
            } else if file_type.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "yaml")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| !name.starts_with('_'))
            {
                let document: PersistedRequest = read_yaml(&path)?;
                nodes.push(LocatedNode {
                    node: WorkspaceNode {
                        id: document.id,
                        parent_id,
                        depth,
                        name: document.name,
                        method: Some(document.spec.method),
                        is_folder: false,
                        expanded: false,
                        kind: WorkspaceNodeKind::Request,
                        debug_case_count: document.debug_cases.len(),
                    },
                    path,
                });
            }
        }
        Ok(())
    }

    fn locate(&self, id: Uuid) -> Result<LocatedNode> {
        self.scan()?
            .into_iter()
            .find(|item| item.node.id == id)
            .with_context(|| format!("未知的工作区节点：{id}"))
    }

    fn collection_path(&self, entry: &CollectionEntry) -> Result<PathBuf> {
        let relative = PathBuf::from(&entry.directory);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
        {
            bail!("集合目录包含非法路径：{}", entry.directory);
        }
        Ok(self.root.join(relative))
    }

    fn read_environments(&self, manifest: &WorkspaceManifest) -> Result<Vec<Environment>> {
        manifest
            .environments
            .iter()
            .map(|entry| {
                let environment: Environment = read_yaml(&self.environment_path(entry.id))
                    .with_context(|| {
                        format!(
                            "读取环境文件失败：{}",
                            self.environment_path(entry.id).display()
                        )
                    })?;
                if environment.id != entry.id {
                    bail!(
                        "环境文件 {} 的 id 与工作区清单不一致",
                        self.environment_path(entry.id).display()
                    );
                }
                Ok(environment)
            })
            .collect()
    }

    fn environment_path(&self, id: Uuid) -> PathBuf {
        self.root.join(ENVIRONMENT_DIR).join(format!("{id}.yaml"))
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join("apogee.yaml")
    }

    fn read_manifest(&self) -> Result<WorkspaceManifest> {
        let manifest: WorkspaceManifest = read_yaml(&self.manifest_path())
            .with_context(|| format!("读取工作区清单失败：{}", self.manifest_path().display()))?;
        if manifest.apogee != WORKSPACE_FORMAT_VERSION {
            bail!(
                "不支持的工作区格式版本：{}（当前支持 {}）",
                manifest.apogee,
                WORKSPACE_FORMAT_VERSION
            );
        }
        Ok(manifest)
    }
}

fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn default_request_spec() -> RequestSpec {
    RequestSpec {
        method: "GET".to_string(),
        url: String::new(),
        params: Vec::new(),
        path_params: Vec::new(),
        headers: Vec::new(),
        body: Body::None,
        auth: AuthConfig::Inherit,
        settings: RequestSettings::default(),
    }
}

fn container_marker(kind: &WorkspaceNodeKind) -> Result<&'static str> {
    match kind {
        WorkspaceNodeKind::Collection => Ok(COLLECTION_MARKER),
        WorkspaceNodeKind::Folder => Ok(FOLDER_MARKER),
        WorkspaceNodeKind::Request => bail!("请求节点没有容器配置"),
    }
}

fn merge_settings(base: &mut RequestSettings, override_with: &RequestSettings) {
    if override_with.timeout_ms.is_some() {
        base.timeout_ms = override_with.timeout_ms;
    }
    if override_with.follow_redirects.is_some() {
        base.follow_redirects = override_with.follow_redirects;
    }
    if override_with.max_redirects.is_some() {
        base.max_redirects = override_with.max_redirects;
    }
    if override_with.verify_tls.is_some() {
        base.verify_tls = override_with.verify_tls;
    }
    if override_with.http_version.is_some() {
        base.http_version = override_with.http_version;
    }
    if override_with.encode_url.is_some() {
        base.encode_url = override_with.encode_url;
    }
    if override_with.send_cookies.is_some() {
        base.send_cookies = override_with.send_cookies;
    }
    if override_with.proxy_url.is_some() {
        base.proxy_url = override_with.proxy_url.clone();
    }
    if override_with.retry_count.is_some() {
        base.retry_count = override_with.retry_count;
    }
    if override_with.retry_backoff_ms.is_some() {
        base.retry_backoff_ms = override_with.retry_backoff_ms;
    }
}

fn unique_name<'a>(base: &str, existing: impl IntoIterator<Item = &'a str>) -> String {
    let base = base.trim();
    let base = if base.is_empty() { "YApi 项目" } else { base };
    let names: std::collections::HashSet<&str> = existing.into_iter().collect();
    if !names.contains(base) {
        return base.to_string();
    }
    for number in 2.. {
        let candidate = format!("{base} ({number})");
        if !names.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!()
}

fn validate_import_tree(root: &Path) -> Result<()> {
    let _: ContainerDocument = read_yaml(&root.join(COLLECTION_MARKER))?;
    fn visit(directory: &Path) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                let _: ContainerDocument = read_yaml(&path.join(FOLDER_MARKER))?;
                visit(&path)?;
            } else if path
                .extension()
                .is_some_and(|extension| extension == "yaml")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| !name.starts_with('_'))
            {
                let _: PersistedRequest = read_yaml(&path)?;
            }
        }
        Ok(())
    }
    visit(root)
}

fn required_name(name: &str) -> Result<String> {
    let value = name.trim();
    if value.is_empty() {
        bail!("名称不能为空");
    }
    if value.chars().count() > 120 {
        bail!("名称不能超过 120 个字符");
    }
    Ok(value.to_string())
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remove_directory_if_exists(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn read_yaml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let reader = fs::File::open(path)?;
    Ok(apogee_core::yaml::from_reader(reader)?)
}

fn write_yaml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let contents = apogee_core::yaml::to_string(value)?;
    atomic_write(path, contents.as_bytes())
}

/// 兼容现有 YAML 调用点，同时统一委托给带 `sync_all` 的共享发布基础。
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::persistence::atomic_write(path, bytes)
}

/// 为可控故障测试保留发布边界；生产路径始终传入共享平台原子替换实现。
#[cfg(test)]
fn atomic_write_with_replace(
    path: &Path,
    bytes: &[u8],
    replace: impl FnOnce(&Path, &Path) -> Result<()>,
) -> Result<()> {
    crate::persistence::atomic_write_with_replace(path, bytes, replace)
}

#[cfg(test)]
#[allow(
    clippy::items_after_test_module,
    reason = "normalize_environment_host is kept after the focused persistence regressions to avoid a broad unrelated reorder"
)]
mod tests {
    use super::*;
    use apogee_core::model::{ActualRequest, Timing, ValueKind};
    use apogee_core::public::{
        PublicError, PublicHint, PublicRequestPreview, PublicResponseSummary,
    };

    #[test]
    fn opening_plain_directory_does_not_initialize_it() {
        let root = std::env::temp_dir().join(format!("apogee-open-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(WorkspaceRepository::open(&root).is_err());
        assert!(!root.join("apogee.yaml").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn yaml_codec_failure_preserves_file_and_creates_no_publish_temporary_file() {
        struct Failure;
        impl Serialize for Failure {
            fn serialize<S: serde::Serializer>(
                &self,
                _: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("fixture-private-secret"))
            }
        }
        let root =
            std::env::temp_dir().join(format!("apogee-yaml-codec-failure-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("fixture.yaml");
        fs::write(&target, b"previous: intact\n").unwrap();
        let error = write_yaml(&target, &Failure).unwrap_err();
        assert!(!format!("{error:#}").contains("fixture-private-secret"));
        assert_eq!(fs::read(&target).unwrap(), b"previous: intact\n");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_history_yaml_publish_preserves_existing_record_and_cleans_temporary_file() {
        let root =
            std::env::temp_dir().join(format!("apogee-history-write-test-{}", Uuid::new_v4()));
        let history = root.join(HISTORY_DIR);
        fs::create_dir_all(&history).unwrap();
        let target = history.join("record.yaml");
        let previous = b"history: previous\n";
        fs::write(&target, previous).unwrap();

        let error = atomic_write_with_replace(&target, b"history: replacement\n", |_, _| {
            anyhow::bail!("injected history publish failure")
        })
        .unwrap_err();

        assert!(format!("{error:#}").contains("injected history publish failure"));
        assert_eq!(fs::read(&target).unwrap(), previous);
        assert!(fs::read_dir(&history).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_environment_updates_manifest_and_removes_file() {
        let root = std::env::temp_dir().join(format!("apogee-environment-test-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let environment = repository.create_environment("待删除".into()).unwrap();
        let environment_path = repository.environment_path(environment.id);

        let snapshot = repository.delete_environment(environment.id).unwrap();

        assert!(!environment_path.exists());
        assert!(!snapshot
            .environments
            .iter()
            .any(|item| item.id == environment.id));
        assert!(repository.read_environment(environment.id).is_err());
        let transaction_directory = root.join(TRANSACTION_DIR);
        assert!(
            !transaction_directory.exists()
                || fs::read_dir(&transaction_directory)
                    .unwrap()
                    .next()
                    .is_none(),
            "已提交删除不得遗留可恢复 journal"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_environment_manifest_publish_rolls_back_staging_and_journal() {
        let root = std::env::temp_dir().join(format!(
            "apogee-environment-create-rollback-test-{}",
            Uuid::new_v4()
        ));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let environment_directory = root.join(ENVIRONMENT_DIR);
        let file_count_before = fs::read_dir(&environment_directory).unwrap().count();

        let error = repository
            .create_environment_with_manifest_writer("不会提交".into(), |_, _| {
                anyhow::bail!("injected manifest publish failure")
            })
            .unwrap_err();

        assert!(format!("{error:#}").contains("injected manifest publish failure"));
        assert_eq!(repository.read_manifest().unwrap().environments.len(), 1);
        assert_eq!(
            fs::read_dir(&environment_directory).unwrap().count(),
            file_count_before
        );
        let transaction_directory = root.join(TRANSACTION_DIR);
        assert!(
            !transaction_directory.exists()
                || fs::read_dir(&transaction_directory)
                    .unwrap()
                    .next()
                    .is_none(),
            "失败创建不得遗留 staging 或 journal"
        );
        fs::remove_dir_all(root).unwrap();
    }

    struct CollectionTransactionFixture {
        root: PathBuf,
        repository: WorkspaceRepository,
        id: Uuid,
        request_id: Uuid,
    }

    impl CollectionTransactionFixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("apogee-collection-tx-test-{}", Uuid::new_v4()));
            let repository = WorkspaceRepository::create(&root, "我的集合".into()).unwrap();
            let id = repository
                .snapshot()
                .unwrap()
                .nodes
                .iter()
                .find(|node| matches!(node.kind, WorkspaceNodeKind::Collection))
                .unwrap()
                .id;
            let folder = repository
                .create_folder(id, "保留的请求".into())
                .unwrap()
                .nodes
                .into_iter()
                .find(|node| matches!(node.kind, WorkspaceNodeKind::Folder))
                .unwrap()
                .id;
            let request_id = repository
                .create_request(folder, "保留的请求".into())
                .unwrap()
                .id;
            Self {
                root,
                repository,
                id,
                request_id,
            }
        }

        fn journal(&self, rename: bool) -> PathBuf {
            self.repository
                .begin_transaction(if rename {
                    WorkspaceTransactionOperation::CollectionRename {
                        collection_id: self.id,
                    }
                } else {
                    WorkspaceTransactionOperation::CollectionDelete {
                        collection_id: self.id,
                    }
                })
                .unwrap()
        }

        fn commit_delete(&self) {
            let mut manifest = self.repository.read_manifest().unwrap();
            manifest.collections.retain(|entry| entry.id != self.id);
            write_yaml(&self.repository.manifest_path(), &manifest).unwrap();
        }

        fn assert_name(&self, expected: &str) {
            let manifest = self.repository.read_manifest().unwrap();
            assert_eq!(
                manifest
                    .collections
                    .iter()
                    .find(|entry| entry.id == self.id)
                    .unwrap()
                    .name,
                expected
            );
            let document: ContainerDocument = read_yaml(
                &self
                    .root
                    .join(format!("collections/{}/{}", self.id, COLLECTION_MARKER)),
            )
            .unwrap();
            assert_eq!(document.name, expected);
            assert_eq!(
                self.repository.read_request(self.request_id).unwrap().name,
                "保留的请求"
            );
        }

        fn assert_no_journal(&self) {
            let directory = self.repository.transaction_directory();
            assert!(!directory.exists() || fs::read_dir(directory).unwrap().next().is_none());
        }
    }

    impl Drop for CollectionTransactionFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn collection_delete_removes_subtree_only_after_manifest_commit() {
        let fixture = CollectionTransactionFixture::new();
        let environments = fixture.repository.snapshot().unwrap().environments.len();
        let snapshot = fixture.repository.delete_node(fixture.id).unwrap();
        assert!(snapshot.nodes.is_empty());
        assert_eq!(snapshot.environments.len(), environments);
        let (target, tombstone) = fixture
            .repository
            .collection_transaction_paths(fixture.id)
            .unwrap();
        assert!(!target.exists());
        assert!(!tombstone.exists());
        fixture.assert_no_journal();
    }

    #[test]
    fn failed_collection_delete_manifest_publish_preserves_nested_requests() {
        let fixture = CollectionTransactionFixture::new();
        let manifest_before = fs::read(fixture.repository.manifest_path()).unwrap();
        let error = fixture
            .repository
            .delete_collection_with_manifest_writer(fixture.id, |_, _| {
                let (target, tombstone) = fixture
                    .repository
                    .collection_transaction_paths(fixture.id)?;
                assert!(target.is_dir());
                assert!(!tombstone.exists());
                assert!(fixture.repository.read_request(fixture.request_id).is_ok());
                bail!("injected collection manifest publish failure")
            })
            .unwrap_err();
        assert!(format!("{error:#}").contains("injected collection manifest publish failure"));
        assert_eq!(
            fs::read(fixture.repository.manifest_path()).unwrap(),
            manifest_before
        );
        fixture.assert_name("我的集合");
        fixture.assert_no_journal();
    }

    #[test]
    fn collection_delete_recovery_preserves_uncommitted_subtree() {
        for moved in [false, true] {
            let fixture = CollectionTransactionFixture::new();
            let journal = fixture.journal(false);
            let (target, tombstone) = fixture
                .repository
                .collection_transaction_paths(fixture.id)
                .unwrap();
            if moved {
                fs::rename(&target, &tombstone).unwrap();
            }
            fixture.repository.recover_pending_transactions().unwrap();
            fixture.repository.recover_pending_transactions().unwrap();
            fixture.assert_name("我的集合");
            assert!(target.is_dir());
            assert!(!tombstone.exists());
            assert!(!journal.exists());
        }
    }

    #[test]
    fn collection_delete_recovery_completes_each_post_commit_cleanup_stage() {
        for stage in 0..3 {
            let fixture = CollectionTransactionFixture::new();
            let journal = fixture.journal(false);
            let (target, tombstone) = fixture
                .repository
                .collection_transaction_paths(fixture.id)
                .unwrap();
            fixture.commit_delete();
            if stage >= 1 {
                fs::rename(&target, &tombstone).unwrap();
            }
            if stage == 2 {
                fs::remove_dir_all(&tombstone).unwrap();
            }
            fixture.repository.recover_pending_transactions().unwrap();
            fixture.repository.recover_pending_transactions().unwrap();
            assert!(fixture.repository.snapshot().unwrap().nodes.is_empty());
            assert!(!target.exists());
            assert!(!tombstone.exists());
            assert!(!journal.exists());
        }
    }

    #[test]
    fn collection_delete_recovery_rejects_conflicting_uncommitted_directories() {
        let fixture = CollectionTransactionFixture::new();
        let journal = fixture.journal(false);
        let (target, tombstone) = fixture
            .repository
            .collection_transaction_paths(fixture.id)
            .unwrap();
        fs::create_dir(&tombstone).unwrap();
        assert!(fixture.repository.recover_pending_transactions().is_err());
        assert!(target.is_dir());
        assert!(tombstone.is_dir());
        assert!(journal.exists());
        fixture.assert_name("我的集合");
    }

    #[test]
    fn collection_delete_recovery_refuses_directory_referenced_under_another_id() {
        let fixture = CollectionTransactionFixture::new();
        let journal = fixture.journal(false);
        let mut manifest = fixture.repository.read_manifest().unwrap();
        manifest.collections[0].id = Uuid::new_v4();
        write_yaml(&fixture.repository.manifest_path(), &manifest).unwrap();
        assert!(fixture.repository.recover_pending_transactions().is_err());
        assert!(fixture.repository.read_request(fixture.request_id).is_ok());
        assert!(journal.exists());
    }

    #[test]
    fn collection_delete_rejects_alias_before_manifest_commit() {
        let fixture = CollectionTransactionFixture::new();
        let mut manifest = fixture.repository.read_manifest().unwrap();
        let mut alias = manifest.collections[0].clone();
        alias.id = Uuid::new_v4();
        alias.directory = format!("./collections/{}", fixture.id);
        manifest.collections.push(alias);
        write_yaml(&fixture.repository.manifest_path(), &manifest).unwrap();
        let before = fs::read(fixture.repository.manifest_path()).unwrap();
        assert!(fixture
            .repository
            .delete_collection_with_manifest_writer(fixture.id, |_, _| {
                panic!("aliased directories must not reach the commit point")
            })
            .is_err());
        assert_eq!(
            fs::read(fixture.repository.manifest_path()).unwrap(),
            before
        );
        assert!(fixture.repository.read_request(fixture.request_id).is_ok());
        fixture.assert_no_journal();
    }

    #[test]
    fn collection_rename_recovery_preserves_journal_on_identity_mismatch() {
        let fixture = CollectionTransactionFixture::new();
        let journal = fixture.journal(true);
        let marker = fixture
            .root
            .join(format!("collections/{}/{}", fixture.id, COLLECTION_MARKER));
        let mut document: ContainerDocument = read_yaml(&marker).unwrap();
        document.id = Uuid::new_v4();
        write_yaml(&marker, &document).unwrap();
        let before = fs::read(&marker).unwrap();
        assert!(fixture.repository.recover_pending_transactions().is_err());
        assert_eq!(fs::read(marker).unwrap(), before);
        assert!(journal.exists());
    }

    #[cfg(unix)]
    #[test]
    fn collection_delete_recovery_refuses_symlink_to_another_directory() {
        let fixture = CollectionTransactionFixture::new();
        let journal = fixture.journal(false);
        let (target, _) = fixture
            .repository
            .collection_transaction_paths(fixture.id)
            .unwrap();
        let retained = fixture.root.join("retained-collection");
        fs::rename(&target, &retained).unwrap();
        std::os::unix::fs::symlink(&retained, &target).unwrap();
        fixture.commit_delete();
        assert!(fixture.repository.recover_pending_transactions().is_err());
        assert!(retained.join(COLLECTION_MARKER).is_file());
        assert!(journal.exists());
    }

    #[test]
    fn collection_transactions_reject_nonstandard_paths_without_mutation() {
        let fixture = CollectionTransactionFixture::new();
        let mut manifest = fixture.repository.read_manifest().unwrap();
        manifest.collections[0].directory = "collections".into();
        write_yaml(&fixture.repository.manifest_path(), &manifest).unwrap();
        let before = fs::read(fixture.repository.manifest_path()).unwrap();
        assert!(fixture
            .repository
            .delete_collection_with_manifest_writer(fixture.id, |_, _| {
                panic!("invalid directory must be rejected before publishing")
            })
            .is_err());
        assert_eq!(
            fs::read(fixture.repository.manifest_path()).unwrap(),
            before
        );
        assert!(fixture
            .root
            .join(format!("collections/{}", fixture.id))
            .is_dir());
        fixture.assert_no_journal();
    }

    #[test]
    fn collection_delete_error_after_commit_never_restores_manifest_reference() {
        let fixture = CollectionTransactionFixture::new();
        assert!(fixture
            .repository
            .delete_collection_with_manifest_writer(fixture.id, |path, manifest| {
                write_yaml(path, manifest)?;
                bail!("injected error after commit")
            })
            .is_err());
        assert!(fixture.repository.snapshot().unwrap().nodes.is_empty());
        assert!(!fixture
            .root
            .join(format!("collections/{}", fixture.id))
            .exists());
        fixture.assert_no_journal();
    }

    #[test]
    fn collection_rename_commits_both_names_and_preserves_container_configuration() {
        let fixture = CollectionTransactionFixture::new();
        let mut container = fixture.repository.read_container(fixture.id).unwrap();
        container.auth = AuthConfig::Bearer {
            token: "{{token}}".into(),
        };
        container.settings.timeout_ms = Some(1234);
        fixture
            .repository
            .save_container(container.clone())
            .unwrap();
        fixture
            .repository
            .rename_node(fixture.id, " 新名称 ".into())
            .unwrap();
        fixture.assert_name("新名称");
        let saved = fixture.repository.read_container(fixture.id).unwrap();
        assert_eq!(
            serde_json::to_value(saved.auth).unwrap(),
            serde_json::to_value(container.auth).unwrap()
        );
        assert_eq!(saved.settings.timeout_ms, Some(1234));
        fixture.assert_no_journal();
    }

    #[test]
    fn failed_collection_rename_manifest_publish_restores_old_name() {
        let fixture = CollectionTransactionFixture::new();
        let marker = fixture
            .root
            .join(format!("collections/{}/{}", fixture.id, COLLECTION_MARKER));
        let before = fs::read(&marker).unwrap();
        let error = fixture
            .repository
            .rename_collection_with_manifest_writer(fixture.id, "保留的请求".into(), |_, _| {
                let document: ContainerDocument = read_yaml(&marker)?;
                assert_eq!(document.name, "保留的请求");
                bail!("injected collection rename publish failure")
            })
            .unwrap_err();
        assert!(format!("{error:#}").contains("injected collection rename publish failure"));
        assert_eq!(fs::read(marker).unwrap(), before);
        fixture.assert_name("我的集合");
        fixture.assert_no_journal();
    }

    #[test]
    fn collection_rename_recovery_uses_manifest_as_commit_point() {
        for committed in [false, true] {
            let fixture = CollectionTransactionFixture::new();
            let journal = fixture.journal(true);
            let marker = fixture
                .root
                .join(format!("collections/{}/{}", fixture.id, COLLECTION_MARKER));
            let mut document: ContainerDocument = read_yaml(&marker).unwrap();
            document.name = "新名称".into();
            write_yaml(&marker, &document).unwrap();
            if committed {
                let mut manifest = fixture.repository.read_manifest().unwrap();
                manifest.collections[0].name = "新名称".into();
                write_yaml(&fixture.repository.manifest_path(), &manifest).unwrap();
            }
            fixture.repository.recover_pending_transactions().unwrap();
            fixture.repository.recover_pending_transactions().unwrap();
            fixture.assert_name(if committed {
                "新名称"
            } else {
                "我的集合"
            });
            assert!(!journal.exists());
        }
    }

    #[test]
    fn collection_rename_error_after_commit_preserves_new_name() {
        let fixture = CollectionTransactionFixture::new();
        assert!(fixture
            .repository
            .rename_collection_with_manifest_writer(
                fixture.id,
                "保留的请求".into(),
                |path, manifest| {
                    write_yaml(path, manifest)?;
                    bail!("injected error after rename commit")
                }
            )
            .is_err());
        fixture.assert_name("保留的请求");
        fixture.assert_no_journal();
    }

    #[test]
    fn collection_rename_recovery_keeps_journal_when_document_is_missing() {
        let fixture = CollectionTransactionFixture::new();
        let journal = fixture.journal(true);
        fs::remove_file(
            fixture
                .root
                .join(format!("collections/{}/{}", fixture.id, COLLECTION_MARKER)),
        )
        .unwrap();
        assert!(fixture.repository.recover_pending_transactions().is_err());
        assert!(journal.exists());
    }

    #[test]
    fn failed_environment_delete_manifest_publish_preserves_file_and_cleans_journal() {
        let root = std::env::temp_dir().join(format!(
            "apogee-environment-delete-rollback-test-{}",
            Uuid::new_v4()
        ));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let environment = repository.create_environment("不会删除".into()).unwrap();
        let target = repository.environment_path(environment.id);
        let tombstone = repository.environment_delete_tombstone_path(environment.id);
        let target_before_publish = target.clone();
        let tombstone_before_publish = tombstone.clone();

        let error = repository
            .delete_environment_with_manifest_writer(environment.id, move |_, _| {
                assert!(
                    target_before_publish.is_file(),
                    "manifest 提交前读取者必须仍能找到环境文件"
                );
                assert!(
                    !tombstone_before_publish.exists(),
                    "manifest 提交前不得移动环境文件到 tombstone"
                );
                anyhow::bail!("injected manifest publish failure")
            })
            .unwrap_err();

        assert!(format!("{error:#}").contains("injected manifest publish failure"));
        assert!(target.is_file(), "删除失败必须保留原环境文件");
        assert!(!tombstone.exists(), "删除失败不得遗留 tombstone");
        assert!(
            repository
                .read_manifest()
                .unwrap()
                .environments
                .iter()
                .any(|entry| entry.id == environment.id),
            "删除失败不得发布移除环境的 manifest"
        );
        assert_eq!(
            repository.read_environment(environment.id).unwrap().name,
            "不会删除"
        );
        let transaction_directory = root.join(TRANSACTION_DIR);
        assert!(
            !transaction_directory.exists()
                || fs::read_dir(&transaction_directory)
                    .unwrap()
                    .next()
                    .is_none(),
            "失败删除不得遗留 tombstone 或 journal"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_completes_committed_environment_delete_before_file_cleanup() {
        let root = std::env::temp_dir().join(format!(
            "apogee-environment-delete-commit-recovery-test-{}",
            Uuid::new_v4()
        ));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let environment = repository.create_environment("已提交删除".into()).unwrap();
        let target = repository.environment_path(environment.id);
        let journal = repository
            .begin_transaction(WorkspaceTransactionOperation::EnvironmentDelete {
                environment_id: environment.id,
            })
            .unwrap();
        let mut manifest = repository.read_manifest().unwrap();
        manifest
            .environments
            .retain(|entry| entry.id != environment.id);
        write_yaml(&repository.manifest_path(), &manifest).unwrap();

        repository.recover_pending_transactions().unwrap();

        assert!(!target.exists(), "已提交删除的遗留目标文件必须被恢复清理");
        assert!(!journal.exists());
        assert!(!repository
            .read_manifest()
            .unwrap()
            .environments
            .iter()
            .any(|entry| entry.id == environment.id));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_removes_published_environment_create_before_manifest_commit() {
        let root = std::env::temp_dir().join(format!(
            "apogee-environment-create-recovery-test-{}",
            Uuid::new_v4()
        ));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let environment = Environment {
            id: Uuid::new_v4(),
            name: "中断创建".into(),
            host: String::new(),
            variables: Vec::new(),
        };
        let journal = repository
            .begin_transaction(WorkspaceTransactionOperation::EnvironmentCreate {
                environment_id: environment.id,
            })
            .unwrap();
        let staging = repository.environment_create_staging_path(environment.id);
        let target = repository.environment_path(environment.id);
        write_yaml(&staging, &environment).unwrap();
        fs::rename(&staging, &target).unwrap();

        repository.recover_pending_transactions().unwrap();

        assert!(!staging.exists());
        assert!(!target.exists());
        assert!(!journal.exists());
        assert!(!repository
            .read_manifest()
            .unwrap()
            .environments
            .iter()
            .any(|entry| entry.id == environment.id));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_restores_uncommitted_environment_delete() {
        let root = std::env::temp_dir().join(format!(
            "apogee-environment-delete-recovery-test-{}",
            Uuid::new_v4()
        ));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let environment = repository.create_environment("中断删除".into()).unwrap();
        let target = repository.environment_path(environment.id);
        let tombstone = repository.environment_delete_tombstone_path(environment.id);
        let journal = repository
            .begin_transaction(WorkspaceTransactionOperation::EnvironmentDelete {
                environment_id: environment.id,
            })
            .unwrap();
        fs::rename(&target, &tombstone).unwrap();

        repository.recover_pending_transactions().unwrap();

        assert!(target.is_file());
        assert!(!tombstone.exists());
        assert!(!journal.exists());
        assert_eq!(
            repository.read_environment(environment.id).unwrap().name,
            "中断删除"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn environment_host_is_validated_and_normalized_on_save() {
        let root =
            std::env::temp_dir().join(format!("apogee-environment-host-test-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let mut environment = repository.create_environment("开发".into()).unwrap();

        environment.host = "https://api.example.test/".into();
        let saved = repository.save_environment(environment.clone()).unwrap();
        assert_eq!(saved.host, "https://api.example.test");

        environment.host = "https://api.example.test/v1".into();
        assert!(repository.save_environment(environment).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_round_trip_preserves_public_request_response_and_body() {
        let root = std::env::temp_dir().join(format!("apogee-history-test-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let response_id = Uuid::new_v4();
        let source = root.join("source.body");
        fs::write(&source, br#"{"ok":true}"#).unwrap();
        let mut response = ResponseSummary::without_http(
            response_id,
            11,
            Timing {
                total_ms: 12.5,
                ..Default::default()
            },
        );
        response.status = Some(200);
        let spec = default_request_spec();

        repository
            .record_history(
                1234,
                PublicRequestPreview::from(&spec),
                PublicResponseSummary::from(&response),
                &source,
                None,
                None,
            )
            .unwrap();
        let listed = repository.list_history().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, Some(200));
        let (record, body) = repository.read_history(response_id).unwrap();
        assert_eq!(record.created_at_ms, 1234);
        assert_eq!(record.spec.method, "GET");
        assert_eq!(
            fs::read(body.expect("成功历史必须有响应体")).unwrap(),
            br#"{"ok":true}"#
        );

        repository.clear_history().unwrap();
        assert!(repository.list_history().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_history_round_trip_preserves_safe_error_without_body() {
        let root =
            std::env::temp_dir().join(format!("apogee-failed-history-test-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();

        repository
            .record_failed_history(FailedHistoryWriteInput {
                created_at_ms: 1234,
                spec: PublicRequestPreview::from(&default_request_spec()),
                error: PublicError {
                    code: Some("CONNECT_FAILED".into()),
                    kind: "connect".into(),
                    message: "任何输入消息都会在写入时转换为安全通用消息".into(),
                    hints: vec![PublicHint {
                        label: "确认服务正在运行后重试".into(),
                        action: Some("retry".into()),
                    }],
                },
                actual_request: None,
                duration_ms: 42.5,
                debug_case_id: None,
                environment_id: None,
            })
            .unwrap();

        let listed = repository.list_history().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, None);
        assert!((listed[0].duration_ms - 42.5).abs() < f64::EPSILON);

        let (record, body) = repository.read_history(listed[0].id).unwrap();
        assert_eq!(record.created_at_ms, 1234);
        assert!(record.response.is_none());
        assert!(record.actual_request.is_none());
        assert!(body.is_none());
        let error = record.error.expect("失败历史必须保留错误");
        assert_eq!(error.code.as_deref(), Some("CONNECT_FAILED"));
        assert_eq!(error.kind, "connect");
        assert_eq!(error.message, "无法连接到服务器");
        assert_eq!(error.hints.len(), 1);
        assert_eq!(error.hints[0].action.as_deref(), Some("retry"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_yaml_persists_only_public_dto_fields() {
        let root = std::env::temp_dir().join(format!(
            "apogee-public-history-write-test-{}",
            Uuid::new_v4()
        ));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let response_id = Uuid::new_v4();
        let source = root.join("source.body");
        fs::write(&source, b"ok").unwrap();
        let mut spec = default_request_spec();
        spec.method = "POST".into();
        spec.url = "https://writer:writer-password@example.test/v1/path-secret?api_key=query-secret&safe=ok".into();
        spec.params = vec![KeyValue {
            enabled: true,
            key: "token".into(),
            value: "param-secret".into(),
            description: Some("description-secret".into()),
            kind: ValueKind::Secret,
        }];
        spec.path_params = vec![KeyValue {
            enabled: true,
            key: "secret".into(),
            value: "path-secret".into(),
            description: None,
            kind: ValueKind::Text,
        }];
        spec.headers = vec![
            KeyValue {
                enabled: true,
                key: "Authorization".into(),
                value: "Bearer header-secret".into(),
                description: None,
                kind: ValueKind::Text,
            },
            KeyValue {
                enabled: true,
                key: "X-Trace".into(),
                value: "safe-header".into(),
                description: None,
                kind: ValueKind::Text,
            },
        ];
        spec.body = Body::Json {
            content: "body-secret".into(),
        };
        spec.auth = AuthConfig::Bearer {
            token: "auth-secret".into(),
        };
        spec.settings = RequestSettings {
            proxy_url: Some("http://proxy-user:proxy-secret@proxy.test:8080".into()),
            ..Default::default()
        };
        let mut response = ResponseSummary::without_http(response_id, 2, Timing::default());
        response.status = Some(201);
        response.headers = vec![
            ("Set-Cookie".into(), "session=response-cookie-secret".into()),
            ("X-Request-Id".into(), "safe-response-header".into()),
        ];
        response.actual_request = Some(ActualRequest {
            method: "POST".into(),
            url: "https://writer:writer-password@example.test/v1/path-secret/a%20b?token=actual-query-secret&q=actual-neutral-query-secret".into(),
            path_params: vec![
                ("secret".into(), "path-secret".into()),
                ("id".into(), "a b".into()),
            ],
            headers: vec![
                ("Cookie".into(), "session=actual-cookie-secret".into()),
                ("X-My-Key".into(), "actual-neutral-header-secret".into()),
            ],
            common_header_names: Vec::new(),
            body: Some("actual-body-secret".into()),
            body_params: vec![("password".into(), "actual-body-param-secret".into())],
            sensitive_path_param_names: vec!["id".into()],
            sensitive_path_values: vec!["a b".into()],
            sensitive_query_names: vec!["q".into()],
            sensitive_header_names: vec!["X-My-Key".into()],
        });

        repository
            .record_history(
                1234,
                PublicRequestPreview::from(&spec),
                PublicResponseSummary::from(&response),
                &source,
                None,
                None,
            )
            .unwrap();

        let yaml = fs::read_to_string(
            repository
                .history_directory()
                .join(format!("{response_id}.yaml")),
        )
        .unwrap();
        for secret in [
            "writer-password",
            "query-secret",
            "param-secret",
            "description-secret",
            "path-secret",
            "header-secret",
            "body-secret",
            "auth-secret",
            "proxy-secret",
            "response-cookie-secret",
            "actual-query-secret",
            "actual-cookie-secret",
            "actual-neutral-query-secret",
            "actual-neutral-header-secret",
            "a b",
            "a%20b",
            "actual-body-secret",
            "actual-body-param-secret",
        ] {
            assert!(!yaml.contains(secret), "history YAML 泄露了 {secret}");
        }
        assert!(yaml.contains("safe-header"));
        assert!(yaml.contains("safe-response-header"));
        assert!(yaml.contains("status: 201"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_history_is_read_compatibly_but_sanitized_before_output() {
        let root =
            std::env::temp_dir().join(format!("apogee-legacy-history-test-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let id = Uuid::new_v4();
        let history = repository.history_directory();
        fs::create_dir_all(&history).unwrap();
        let path = history.join(format!("{id}.yaml"));
        let legacy = format!(
            r#"id: {id}
createdAtMs: 5678
spec:
  method: POST
  url: https://legacy-user:legacy-password@example.test/v1/legacy-path-secret?token=legacy-query-secret&safe=ok#private
  params:
    - enabled: true
      key: token
      value: legacy-param-secret
      kind: text
    - enabled: true
      key: q
      value: legacy-neutral-query-secret
      kind: secret
  pathParams:
    - enabled: true
      key: secret
      value: legacy-path-secret
      kind: secret
    - enabled: true
      key: id
      value: legacy neutral path
      kind: secret
  headers:
    - enabled: true
      key: Authorization
      value: Bearer legacy-header-secret
      kind: text
    - enabled: true
      key: X-My-Key
      value: legacy-neutral-header-secret
      kind: secret
    - enabled: true
      key: X-Trace
      value: safe-header
      kind: text
  body:
    type: json
    content: legacy-body-secret
  auth:
    type: bearer
    token: legacy-auth-secret
  settings:
    proxyUrl: http://proxy-user:legacy-proxy-secret@proxy.test:8080
error:
  code: CONNECT_FAILED
  kind: connect
  message: legacy-message-secret
  detail: Bearer legacy-detail-secret
  hints:
    - label: 检查 token legacy-hint-secret
actualRequest:
  method: POST
  url: https://legacy-user:legacy-password@example.test/v1/legacy-path-secret/legacy%20neutral%20path?token=legacy-query-secret&q=legacy-neutral-query-secret&safe=ok
  pathParams:
    - - secret
      - legacy-path-secret
    - - id
      - legacy neutral path
  headers:
    - - Cookie
      - session=legacy-cookie-secret
    - - X-My-Key
      - legacy-neutral-header-secret
    - - X-Trace
      - safe-header
  body: legacy-actual-body-secret
  bodyParams:
    - - password
      - legacy-body-param-secret
durationMs: 42.5
"#,
        );
        fs::write(&path, &legacy).unwrap();

        let listed = repository.list_history().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].method, "POST");
        assert_eq!(
            listed[0].url,
            "https://example.test/v1/[redacted]?token=%5Bredacted%5D&safe=ok"
        );
        assert!((listed[0].duration_ms - 42.5).abs() < f64::EPSILON);

        let (record, body) = repository.read_history(id).unwrap();
        assert!(body.is_none());
        assert_eq!(record.spec.params[0].value, apogee_core::public::REDACTED);
        assert_eq!(record.spec.params[1].value, apogee_core::public::REDACTED);
        assert_eq!(record.spec.headers[0].value, apogee_core::public::REDACTED);
        assert_eq!(record.spec.headers[1].value, apogee_core::public::REDACTED);
        assert_eq!(record.spec.headers[2].value, "safe-header");
        assert_eq!(record.error.as_ref().unwrap().message, "无法连接到服务器");
        let actual = record.actual_request.as_ref().unwrap();
        assert_eq!(actual.path_params[0].1, apogee_core::public::REDACTED);
        assert_eq!(actual.path_params[1].1, apogee_core::public::REDACTED);
        assert_eq!(actual.headers[0].1, apogee_core::public::REDACTED);
        assert_eq!(actual.headers[1].1, apogee_core::public::REDACTED);
        let public_json = serde_json::to_string(&record).unwrap();
        for secret in [
            "legacy-password",
            "legacy-query-secret",
            "legacy-param-secret",
            "legacy-path-secret",
            "legacy neutral path",
            "legacy%20neutral%20path",
            "legacy-neutral-query-secret",
            "legacy-neutral-header-secret",
            "legacy-header-secret",
            "legacy-body-secret",
            "legacy-auth-secret",
            "legacy-proxy-secret",
            "legacy-detail-secret",
            "legacy-hint-secret",
            "legacy-cookie-secret",
            "legacy-actual-body-secret",
            "legacy-body-param-secret",
        ] {
            assert!(
                !public_json.contains(secret),
                "公开 history 泄露了 {secret}"
            );
        }
        assert!(!public_json.contains("proxyUrl"));

        // Compatibility is read-only: the original legacy YAML is retained for the user and is
        // not automatically rewritten or deleted merely because it was opened.
        assert_eq!(fs::read_to_string(&path).unwrap(), legacy);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_custom_api_key_sources_redact_top_level_and_embedded_actual_requests() {
        let root = std::env::temp_dir().join(format!(
            "apogee-legacy-custom-api-key-history-test-{}",
            Uuid::new_v4()
        ));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let failed_id = Uuid::new_v4();
        let succeeded_id = Uuid::new_v4();
        let history = repository.history_directory();
        fs::create_dir_all(&history).unwrap();

        let failed_yaml = format!(
            r#"id: {failed_id}
createdAtMs: 1
spec:
  method: GET
  url: https://example.test/failed
  auth:
    type: apiKey
    key: X-My-Key
    value: legacy-header-auth-secret
    location: header
  headers:
    - enabled: true
      key: X-My-Key
      value: legacy-preview-header-secret
      kind: text
actualRequest:
  method: GET
  url: https://example.test/failed
  headers:
    - - X-My-Key
      - manual-header-secret
    - - X-Trace
      - safe-header
durationMs: 1
"#,
        );
        let succeeded_yaml = format!(
            r#"id: {succeeded_id}
createdAtMs: 2
spec:
  method: GET
  url: https://example.test/search
  auth:
    type: apiKey
    key: q
    value: legacy-query-auth-secret
    location: query
response:
  id: {succeeded_id}
  actualRequest:
    method: GET
    url: https://example.test/search?q=embedded-query-secret&safe=ok
    headers:
      - - X-Trace
        - safe-header
durationMs: 2
"#,
        );
        let failed_path = history.join(format!("{failed_id}.yaml"));
        let succeeded_path = history.join(format!("{succeeded_id}.yaml"));
        fs::write(&failed_path, &failed_yaml).unwrap();
        fs::write(&succeeded_path, &succeeded_yaml).unwrap();
        fs::write(history.join(format!("{succeeded_id}.body")), b"ok").unwrap();

        let (failed, failed_body) = repository.read_history(failed_id).unwrap();
        let (succeeded, succeeded_body) = repository.read_history(succeeded_id).unwrap();
        assert!(failed_body.is_none());
        assert!(succeeded_body.is_some());
        assert_eq!(failed.spec.headers[0].value, apogee_core::public::REDACTED);
        assert_eq!(
            failed.actual_request.as_ref().unwrap().headers,
            vec![
                (
                    "X-My-Key".to_string(),
                    apogee_core::public::REDACTED.to_string()
                ),
                ("X-Trace".to_string(), "safe-header".to_string()),
            ]
        );
        assert_eq!(
            succeeded
                .response
                .as_ref()
                .and_then(|response| response.actual_request.as_ref())
                .map(|actual| actual.url.as_str()),
            Some("https://example.test/search?q=%5Bredacted%5D&safe=ok")
        );

        let output = serde_json::to_string(&(failed, succeeded)).unwrap();
        for secret in [
            "legacy-header-auth-secret",
            "legacy-preview-header-secret",
            "manual-header-secret",
            "legacy-query-auth-secret",
            "embedded-query-secret",
        ] {
            assert!(
                !output.contains(secret),
                "公开 legacy history 泄露了 {secret}: {output}"
            );
        }
        assert_eq!(fs::read_to_string(&failed_path).unwrap(), failed_yaml);
        assert_eq!(fs::read_to_string(&succeeded_path).unwrap(), succeeded_yaml);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn example_workspace_covers_supported_methods_bodies_and_auth() {
        let root = std::env::temp_dir().join(format!("apogee-example-test-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create_example(&root).unwrap();
        let requests: Vec<_> = repository
            .snapshot()
            .unwrap()
            .nodes
            .into_iter()
            .filter(|node| matches!(node.kind, WorkspaceNodeKind::Request))
            .map(|node| repository.read_request(node.id).unwrap())
            .collect();
        for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
            assert!(requests.iter().any(|request| request.spec.method == method));
        }
        assert!(requests
            .iter()
            .any(|r| matches!(r.spec.body, Body::Json { .. })));
        assert!(requests
            .iter()
            .any(|r| matches!(r.spec.body, Body::Text { .. })));
        assert!(requests
            .iter()
            .any(|r| matches!(r.spec.body, Body::FormData { .. })));
        assert!(requests
            .iter()
            .any(|r| matches!(r.spec.body, Body::UrlEncoded { .. })));
        assert!(requests
            .iter()
            .any(|r| matches!(r.spec.body, Body::Binary { .. })));
        assert!(requests
            .iter()
            .any(|r| matches!(r.spec.body, Body::GraphQl { .. })));
        assert!(requests
            .iter()
            .any(|r| matches!(r.spec.auth, AuthConfig::Basic { .. })));
        assert!(requests
            .iter()
            .any(|r| matches!(r.spec.auth, AuthConfig::Bearer { .. })));
        assert!(requests
            .iter()
            .any(|r| matches!(r.spec.auth, AuthConfig::ApiKey { .. })));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persists_new_auth_and_resolves_collection_folder_request_overrides() {
        let root = std::env::temp_dir().join(format!("apogee-auth-inherit-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create(&root, "auth".into()).unwrap();
        let collection = repository
            .create_collection("API".into())
            .unwrap()
            .nodes
            .into_iter()
            .find(|n| matches!(n.kind, WorkspaceNodeKind::Collection))
            .unwrap();
        let folder = repository
            .create_folder(collection.id, "Folder".into())
            .unwrap()
            .nodes
            .into_iter()
            .find(|n| {
                n.parent_id == Some(collection.id) && matches!(n.kind, WorkspaceNodeKind::Folder)
            })
            .unwrap();
        let request = repository
            .create_request(folder.id, "Request".into())
            .unwrap();
        for auth in [
            serde_json::json!({"type":"oauth1","consumerKey":"{{key}}","consumerSecret":"{{secret}}"}),
            serde_json::json!({"type":"oauth2","accessToken":"{{token}}"}),
            serde_json::json!({"type":"jwt","secret":"{{secret}}","payload":"{\"sub\":\"{{user}}\"}"}),
        ] {
            let auth: AuthConfig = serde_json::from_value(auth).unwrap();
            repository
                .save_container(WorkspaceContainer {
                    id: collection.id,
                    name: collection.name.clone(),
                    auth: auth.clone(),
                    settings: Default::default(),
                })
                .unwrap();
            let resolved = repository
                .resolve_request_inheritance(request.id, default_request_spec())
                .unwrap();
            assert_eq!(
                serde_json::to_value(&resolved.auth).unwrap(),
                serde_json::to_value(&auth).unwrap()
            );
            let none = repository
                .resolve_request_inheritance(
                    request.id,
                    RequestSpec {
                        auth: AuthConfig::None,
                        ..default_request_spec()
                    },
                )
                .unwrap();
            assert!(matches!(none.auth, AuthConfig::None));
        }
        let folder_auth:AuthConfig=serde_json::from_value(serde_json::json!({"type":"oauth2","accessToken":"folder-token","headerPrefix":"Token"})).unwrap();
        repository
            .save_container(WorkspaceContainer {
                id: folder.id,
                name: folder.name,
                auth: folder_auth,
                settings: Default::default(),
            })
            .unwrap();
        assert!(
            matches!(repository.resolve_request_inheritance(request.id,default_request_spec()).unwrap().auth,AuthConfig::OAuth2 {access_token,..} if access_token=="folder-token")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolves_container_auth_and_settings_from_nearest_ancestor() {
        let root = std::env::temp_dir().join(format!("apogee-inherit-test-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create(&root, "test".into()).unwrap();
        let collection = repository
            .create_collection("API".into())
            .unwrap()
            .nodes
            .into_iter()
            .find(|node| matches!(node.kind, WorkspaceNodeKind::Collection))
            .unwrap();
        let folder = repository
            .create_folder(collection.id, "users".into())
            .unwrap()
            .nodes
            .into_iter()
            .find(|node| {
                node.parent_id == Some(collection.id)
                    && matches!(node.kind, WorkspaceNodeKind::Folder)
            })
            .unwrap();
        let request = repository.create_request(folder.id, "get".into()).unwrap();

        repository
            .save_container(WorkspaceContainer {
                id: collection.id,
                name: collection.name,
                auth: AuthConfig::Bearer {
                    token: "collection".into(),
                },
                settings: RequestSettings {
                    timeout_ms: Some(1000),
                    verify_tls: Some(true),
                    ..Default::default()
                },
            })
            .unwrap();
        repository
            .save_container(WorkspaceContainer {
                id: folder.id,
                name: folder.name,
                auth: AuthConfig::ApiKey {
                    key: "x-key".into(),
                    value: "folder".into(),
                    location: apogee_core::request::ApiKeyIn::Header,
                },
                settings: RequestSettings {
                    timeout_ms: Some(2000),
                    verify_tls: Some(false),
                    ..Default::default()
                },
            })
            .unwrap();

        let spec = repository
            .resolve_request_inheritance(
                request.id,
                RequestSpec {
                    settings: RequestSettings {
                        timeout_ms: Some(3000),
                        ..Default::default()
                    },
                    ..default_request_spec()
                },
            )
            .unwrap();
        assert!(matches!(spec.auth, AuthConfig::ApiKey { ref value, .. } if value == "folder"));
        assert_eq!(spec.settings.timeout_ms, Some(3000));
        assert_eq!(spec.settings.verify_tls, Some(false));
        let token_spec = RequestSpec {
            auth: AuthConfig::None,
            settings: RequestSettings {
                timeout_ms: Some(4000),
                ..Default::default()
            },
            ..default_request_spec()
        };
        let resolved = repository
            .resolve_token_request_inheritance(folder.id, token_spec)
            .unwrap();
        // The edited folder timeout wins over its persisted timeout; only ancestor settings apply.
        assert_eq!(resolved.settings.timeout_ms, Some(4000));
        assert_eq!(resolved.settings.verify_tls, Some(true));
        assert!(matches!(resolved.auth, AuthConfig::None));
        assert!(repository
            .resolve_request_inheritance(folder.id, default_request_spec())
            .is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_history_moves_to_application_runtime_directory() {
        let root =
            std::env::temp_dir().join(format!("apogee-history-migration-{}", Uuid::new_v4()));
        let runtime =
            std::env::temp_dir().join(format!("apogee-runtime-migration-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create(&root, "测试工作区".into()).unwrap();
        let response_id = Uuid::new_v4();
        let source = root.join("source.body");
        fs::write(&source, b"runtime-history").unwrap();
        let legacy_spec = default_request_spec();
        let legacy_response = ResponseSummary::without_http(response_id, 15, Timing::default());
        repository
            .record_history(
                1234,
                PublicRequestPreview::from(&legacy_spec),
                PublicResponseSummary::from(&legacy_response),
                &source,
                None,
                None,
            )
            .unwrap();

        let repository = repository.with_runtime_root(&runtime).unwrap();

        assert!(!root.join(HISTORY_DIR).exists());
        assert!(runtime.join(HISTORY_DIR).is_dir());
        assert_eq!(repository.list_history().unwrap().len(), 1);
        assert_eq!(
            fs::read(
                repository
                    .read_history(response_id)
                    .unwrap()
                    .1
                    .expect("成功历史必须有响应体")
            )
            .unwrap(),
            b"runtime-history"
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(runtime).unwrap();
    }
}

fn normalize_environment_host(host: &str) -> Result<String> {
    let value = host.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    let parsed = url::Url::parse(value).context("Host 必须是完整的 http:// 或 https:// 地址")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("Host 必须是完整的 http:// 或 https:// 地址");
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("Host 只能包含协议、域名和端口，不能包含路径、查询参数或片段");
    }
    Ok(parsed.origin().ascii_serialization())
}
