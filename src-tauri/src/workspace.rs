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
use apogee_core::vars::Environment;
use apogee_core::workspace::{
    CollectionEntry, ContainerDocument, EnvironmentEntry, PersistedRequest, WorkspaceManifest,
    WorkspaceNode, WorkspaceNodeKind, WorkspaceRequest, WorkspaceSnapshot, WORKSPACE_FORMAT_VERSION,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

const COLLECTION_MARKER: &str = "_collection.yaml";
const FOLDER_MARKER: &str = "_folder.yaml";
const ENVIRONMENT_DIR: &str = "environments";

pub struct WorkspaceRepository {
    root: PathBuf,
}

struct LocatedNode {
    node: WorkspaceNode,
    path: PathBuf,
}

impl WorkspaceRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("collections"))?;
        fs::create_dir_all(root.join(ENVIRONMENT_DIR))?;
        let repository = Self { root };
        if repository.manifest_path().exists() {
            repository.read_manifest()?;
        } else {
            repository.create_default_workspace()?;
        }
        Ok(repository)
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
        let environment: Environment = read_yaml(&self.environment_path(id))
            .with_context(|| format!("读取环境文件失败：{}", self.environment_path(id).display()))?;
        if environment.id != id {
            bail!("环境文件 {} 的 id 不一致", self.environment_path(id).display());
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

    fn create_default_workspace(&self) -> Result<()> {
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
                name: "我的工作区".to_string(),
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
                    .with_context(|| format!("读取环境文件失败：{}", self.environment_path(entry.id).display()))?;
                if environment.id != entry.id {
                    bail!("环境文件 {} 的 id 与工作区清单不一致", self.environment_path(entry.id).display());
                }
                Ok(environment)
            })
            .collect()
    }

    fn environment_path(&self, id: Uuid) -> PathBuf {
        self.root
            .join(ENVIRONMENT_DIR)
            .join(format!("{id}.yaml"))
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
