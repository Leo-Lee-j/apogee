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
use apogee_core::request::{AuthConfig, Body, RequestSettings, RequestSpec};
use apogee_core::model::KeyValue;
use apogee_core::response::ResponseSummary;
use apogee_core::vars::Environment;
use apogee_core::workspace::{
    CollectionEntry, ContainerDocument, EnvironmentEntry, PersistedRequest, WorkspaceManifest,
    WorkspaceNode, WorkspaceNodeKind, WorkspaceRequest, WorkspaceSnapshot,
    WORKSPACE_FORMAT_VERSION,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

const COLLECTION_MARKER: &str = "_collection.yaml";
const FOLDER_MARKER: &str = "_folder.yaml";
const ENVIRONMENT_DIR: &str = "environments";
const HISTORY_DIR: &str = "history";
const HISTORY_LIMIT: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: Uuid,
    pub created_at_ms: u64,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub size_bytes: u64,
    pub duration_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: Uuid,
    pub created_at_ms: u64,
    pub spec: RequestSpec,
    pub response: ResponseSummary,
}

pub struct WorkspaceRepository {
    root: PathBuf,
}

struct LocatedNode {
    node: WorkspaceNode,
    path: PathBuf,
}

impl WorkspaceRepository {
    pub fn create_example(root: impl Into<PathBuf>) -> Result<Self> {
        let repository = Self::create(root, "示例工作区".to_string())?;
        let snapshot = repository.create_collection("示例 API 集合".to_string())?;
        let collection = snapshot.nodes.iter().find(|n| matches!(n.kind, WorkspaceNodeKind::Collection)).map(|n| n.id).context("示例集合创建失败")?;
        let folder_snapshot = repository.create_folder(collection, "请求类型示例".to_string())?;
        let folder = folder_snapshot.nodes.iter().find(|n| n.name == "请求类型示例").map(|n| n.id).context("示例文件夹创建失败")?;
        let methods = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
        for method in methods {
            let request = repository.create_request(folder, format!("{method} 请求"))?;
            let mut spec = request.spec;
            spec.method = method.to_string();
            spec.url = "https://httpbin.org/anything".to_string();
            repository.save_request(request.id, request.name, spec)?;
        }
        let bodies = [
            ("JSON 请求体", Body::Json { content: "{\"hello\":\"world\"}".to_string() }),
            ("文本请求体", Body::Text { content: "hello".to_string(), content_type: "text/plain".to_string() }),
            ("表单请求体", Body::FormData { rows: vec![KeyValue { enabled: true, key: "name".to_string(), value: "Apogee".to_string(), description: None, kind: apogee_core::model::ValueKind::Text }] }),
            ("URL 编码请求体", Body::UrlEncoded { rows: vec![KeyValue { enabled: true, key: "name".to_string(), value: "Apogee".to_string(), description: None, kind: apogee_core::model::ValueKind::Text }] }),
            ("GraphQL 请求体", Body::GraphQl { query: "query { anything }".to_string(), variables: "{}".to_string(), operation_name: None }),
            ("二进制请求体模板", Body::Binary { path: PathBuf::new() }),
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
            ("Basic 认证", AuthConfig::Basic { username: "demo".to_string(), password: "password".to_string() }),
            ("Bearer 认证", AuthConfig::Bearer { token: "demo-token".to_string() }),
            ("API Key 认证", AuthConfig::ApiKey { key: "X-API-Key".to_string(), value: "demo-key".to_string(), location: apogee_core::request::ApiKeyIn::Header }),
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
        let repository = Self { root };
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
        fs::create_dir_all(root.join(HISTORY_DIR))?;
        let repository = Self { root };
        repository.create_default_workspace(name)?;
        Ok(repository)
    }

    pub fn root(&self) -> &Path {
        &self.root
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
                },
                path: path.clone(),
            });
            self.scan_children(&path, Some(entry.id), 1, &mut located)?;
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
        spec: RequestSpec,
        response: ResponseSummary,
        body_source: &Path,
    ) -> Result<()> {
        let id = response.id;
        let directory = self.root.join(HISTORY_DIR);
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
                spec,
                response,
            },
        )?;
        self.trim_history()
    }

    pub fn list_history(&self) -> Result<Vec<HistoryEntry>> {
        let mut records = self.read_history_records()?;
        records.sort_by_key(|record| std::cmp::Reverse(record.created_at_ms));
        Ok(records
            .into_iter()
            .map(|record| HistoryEntry {
                id: record.id,
                created_at_ms: record.created_at_ms,
                method: record.spec.method,
                url: record.spec.url,
                status: record.response.status,
                size_bytes: record.response.size_bytes,
                duration_ms: record.response.timing.total_ms,
            })
            .collect())
    }

    pub fn read_history(&self, id: Uuid) -> Result<(HistoryRecord, PathBuf)> {
        let directory = self.root.join(HISTORY_DIR);
        let record: HistoryRecord = read_yaml(&directory.join(format!("{id}.yaml")))?;
        if record.id != id {
            bail!("历史记录 id 不一致：{id}");
        }
        let body = directory.join(format!("{id}.body"));
        if !body.exists() {
            bail!("历史响应体不存在：{id}");
        }
        Ok((record, body))
    }

    pub fn clear_history(&self) -> Result<()> {
        let directory = self.root.join(HISTORY_DIR);
        if directory.exists() {
            fs::remove_dir_all(&directory)?;
        }
        fs::create_dir_all(directory)?;
        Ok(())
    }

    fn read_history_records(&self) -> Result<Vec<HistoryRecord>> {
        let directory = self.root.join(HISTORY_DIR);
        let mut records = Vec::new();
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "yaml")
            {
                records.push(read_yaml(&path)?);
            }
        }
        Ok(records)
    }

    fn trim_history(&self) -> Result<()> {
        let mut records = self.read_history_records()?;
        records.sort_by_key(|record| std::cmp::Reverse(record.created_at_ms));
        for record in records.into_iter().skip(HISTORY_LIMIT) {
            let directory = self.root.join(HISTORY_DIR);
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

    pub fn read_environment(&self, id: Uuid) -> Result<Environment> {
        let manifest = self.read_manifest()?;
        if !manifest.environments.iter().any(|entry| entry.id == id) {
            bail!("未知的环境：{id}");
        }
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
        let name = required_name(&name)?;
        let environment = Environment {
            id: Uuid::new_v4(),
            name: name.clone(),
            variables: Vec::new(),
        };
        write_yaml(&self.environment_path(environment.id), &environment)?;

        let mut manifest = self.read_manifest()?;
        manifest.environments.push(EnvironmentEntry {
            id: environment.id,
            name,
        });
        write_yaml(&self.manifest_path(), &manifest)?;
        Ok(environment)
    }

    pub fn save_environment(&self, mut environment: Environment) -> Result<Environment> {
        environment.name = required_name(&environment.name)?;
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

    pub fn save_request(&self, id: Uuid, name: String, spec: RequestSpec) -> Result<()> {
        let name = required_name(&name)?;
        let location = self.locate(id)?;
        if !matches!(location.node.kind, WorkspaceNodeKind::Request) {
            bail!("节点不是请求：{id}");
        }
        write_yaml(
            &location.path,
            &PersistedRequest {
                apogee: WORKSPACE_FORMAT_VERSION,
                id,
                name,
                spec,
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
        };
        let path = parent.path.join(format!("{id}.yaml"));
        write_yaml(&path, &document)?;
        Ok(document.as_ipc())
    }

    pub fn rename_node(&self, id: Uuid, name: String) -> Result<WorkspaceSnapshot> {
        let name = required_name(&name)?;
        let location = self.locate(id)?;
        match location.node.kind {
            WorkspaceNodeKind::Collection => {
                let mut manifest = self.read_manifest()?;
                let entry = manifest
                    .collections
                    .iter_mut()
                    .find(|entry| entry.id == id)
                    .with_context(|| format!("工作区清单中没有集合：{id}"))?;
                entry.name = name.clone();
                write_yaml(
                    &location.path.join(COLLECTION_MARKER),
                    &ContainerDocument {
                        apogee: WORKSPACE_FORMAT_VERSION,
                        id,
                        name,
                    },
                )?;
                write_yaml(&self.manifest_path(), &manifest)?;
            }
            WorkspaceNodeKind::Folder => {
                write_yaml(
                    &location.path.join(FOLDER_MARKER),
                    &ContainerDocument {
                        apogee: WORKSPACE_FORMAT_VERSION,
                        id,
                        name,
                    },
                )?;
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
        let location = self.locate(id)?;
        match location.node.kind {
            WorkspaceNodeKind::Collection => {
                fs::remove_dir_all(&location.path)
                    .with_context(|| format!("删除集合目录失败：{}", location.path.display()))?;
                let mut manifest = self.read_manifest()?;
                manifest.collections.retain(|entry| entry.id != id);
                write_yaml(&self.manifest_path(), &manifest)?;
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
            },
        )?;
        write_yaml(
            &self.environment_path(environment_id),
            &Environment {
                id: environment_id,
                name: "本地".to_string(),
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
                },
                path: path.clone(),
            });
            self.scan_children(&path, Some(entry.id), 1, &mut nodes)?;
        }
        Ok(nodes)
    }

    fn scan_children(
        &self,
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
                    },
                    path: path.clone(),
                });
                self.scan_children(&path, Some(document.id), depth + 1, nodes)?;
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

fn default_request_spec() -> RequestSpec {
    RequestSpec {
        method: "GET".to_string(),
        url: String::new(),
        params: Vec::new(),
        path_params: Vec::new(),
        headers: Vec::new(),
        body: Body::None,
        auth: AuthConfig::None,
        settings: RequestSettings::default(),
    }
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

fn read_yaml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let contents = fs::read_to_string(path)?;
    Ok(serde_yaml_ng::from_str(&contents)?)
}

fn write_yaml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let contents = serde_yaml_ng::to_string(value)?;
    atomic_write(path, contents.as_bytes())
}

/// 先写同目录临时文件，再替换目标文件，避免崩溃留下半个 YAML。
/// Windows 不能直接 rename 覆盖已有文件，因此仅在替换阶段先移除旧文件。
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("目标文件没有父目录：{}", path.display()))?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace.yaml");
    let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    fs::write(&temporary, bytes)?;

    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_rename_error) if path.exists() => {
            fs::remove_file(path).with_context(|| format!("替换旧文件失败：{}", path.display()))?;
            fs::rename(&temporary, path)
                .with_context(|| format!("发布临时文件失败：{}", path.display()))?;
            Ok(())
        }
        Err(rename_error) => Err(rename_error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apogee_core::model::Timing;

    #[test]
    fn opening_plain_directory_does_not_initialize_it() {
        let root = std::env::temp_dir().join(format!("apogee-open-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(WorkspaceRepository::open(&root).is_err());
        assert!(!root.join("apogee.yaml").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn history_round_trip_preserves_request_response_and_body() {
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
            .record_history(1234, spec, response, &source)
            .unwrap();
        let listed = repository.list_history().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, Some(200));
        let (record, body) = repository.read_history(response_id).unwrap();
        assert_eq!(record.created_at_ms, 1234);
        assert_eq!(fs::read(body).unwrap(), br#"{"ok":true}"#);

        repository.clear_history().unwrap();
        assert!(repository.list_history().unwrap().is_empty());
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
        assert!(requests.iter().any(|r| matches!(r.spec.body, Body::Json { .. })));
        assert!(requests.iter().any(|r| matches!(r.spec.body, Body::Text { .. })));
        assert!(requests.iter().any(|r| matches!(r.spec.body, Body::FormData { .. })));
        assert!(requests.iter().any(|r| matches!(r.spec.body, Body::UrlEncoded { .. })));
        assert!(requests.iter().any(|r| matches!(r.spec.body, Body::Binary { .. })));
        assert!(requests.iter().any(|r| matches!(r.spec.body, Body::GraphQl { .. })));
        assert!(requests.iter().any(|r| matches!(r.spec.auth, AuthConfig::Basic { .. })));
        assert!(requests.iter().any(|r| matches!(r.spec.auth, AuthConfig::Bearer { .. })));
        assert!(requests.iter().any(|r| matches!(r.spec.auth, AuthConfig::ApiKey { .. })));
        fs::remove_dir_all(root).unwrap();
    }
}
