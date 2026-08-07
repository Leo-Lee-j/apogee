use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::workspace::WorkspaceRepository;
use crate::app_settings::clean_windows_path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceDescriptor {
    pub id: Uuid,
    pub name: String,
    pub path: String,
    pub last_opened_at: u64,
    pub status: WorkspaceStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceStatus {
    Available,
    Missing,
    Invalid,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    workspaces: Vec<WorkspaceDescriptor>,
}

pub struct WorkspaceRegistry {
    path: PathBuf,
}

impl WorkspaceRegistry {
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            path: app_data_dir.join("workspaces.json"),
        }
    }

    pub fn list(&self) -> Result<Vec<WorkspaceDescriptor>> {
        let mut registry = self.read()?;
        let mut changed = false;
        for descriptor in &mut registry.workspaces {
            let cleaned = clean_windows_path(&descriptor.path);
            if cleaned != descriptor.path {
                descriptor.path = cleaned;
                changed = true;
            }
            let root = Path::new(&descriptor.path);
            descriptor.status = if !root.exists() {
                WorkspaceStatus::Missing
            } else if root.join("apogee.yaml").is_file() {
                WorkspaceStatus::Available
            } else {
                WorkspaceStatus::Invalid
            };
        }
        registry
            .workspaces
            .sort_by_key(|item| std::cmp::Reverse(item.last_opened_at));
        if changed {
            self.write(&registry)?;
        }
        Ok(registry.workspaces)
    }

    pub fn register(&self, repository: &WorkspaceRepository) -> Result<WorkspaceDescriptor> {
        let root = repository
            .root()
            .canonicalize()
            .with_context(|| format!("无法解析工作区路径：{}", repository.root().display()))?;
        let mut registry = self.read()?;
        let descriptor = WorkspaceDescriptor {
            id: repository.id()?,
            name: repository.name()?,
            path: clean_windows_path(&root.to_string_lossy()),
            last_opened_at: now_ms(),
            status: WorkspaceStatus::Available,
        };
        registry
            .workspaces
            .retain(|item| item.id != descriptor.id && !paths_equal(Path::new(&item.path), &root));
        registry.workspaces.push(descriptor.clone());
        self.write(&registry)?;
        Ok(descriptor)
    }

    pub fn find(&self, id: Uuid) -> Result<WorkspaceDescriptor> {
        self.read()?
            .workspaces
            .into_iter()
            .find(|item| item.id == id)
            .with_context(|| format!("工作区列表中没有该项目：{id}"))
    }

    pub fn forget(&self, id: Uuid) -> Result<()> {
        let mut registry = self.read()?;
        registry.workspaces.retain(|item| item.id != id);
        self.write(&registry)
    }

    fn read(&self) -> Result<Registry> {
        if !self.path.exists() {
            return Ok(Registry::default());
        }
        let bytes = fs::read(&self.path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn write(&self, registry: &Registry) -> Result<()> {
        let parent = self.path.parent().context("工作区注册表没有父目录")?;
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(".workspaces.{}.tmp", Uuid::new_v4()));
        fs::write(&temporary, serde_json::to_vec_pretty(registry)?)?;
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}
