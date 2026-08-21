use crate::app_settings::atomic_replace;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    #[serde(default)]
    pub active_environment_id: Option<Uuid>,
    #[serde(default)]
    pub active_environment_set: bool,
}

pub struct WorkspaceStateRepository {
    root: PathBuf,
}

impl WorkspaceStateRepository {
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            root: app_data_dir.join("workspace-state"),
        }
    }

    pub fn load(&self, workspace_id: Uuid) -> Result<WorkspaceState> {
        let path = self.path(workspace_id);
        if !path.exists() {
            return Ok(WorkspaceState::default());
        }
        let contents = fs::read(&path)?;
        match serde_json::from_slice(&contents) {
            Ok(state) => Ok(state),
            Err(_) => {
                let backup = self
                    .root
                    .join(format!(".{workspace_id}.{}.invalid.json", Uuid::new_v4()));
                fs::rename(&path, &backup)
                    .with_context(|| format!("备份损坏的工作区状态失败：{}", path.display()))?;
                Ok(WorkspaceState::default())
            }
        }
    }

    /// 将状态写入临时文件后发布，避免 Windows 上删除旧状态再发布的空窗。
    pub fn save(&self, workspace_id: Uuid, state: &WorkspaceState) -> Result<()> {
        self.save_with_replace(workspace_id, state, atomic_replace)
    }

    /// 为确定性故障测试保留发布边界；生产路径始终传入 [`atomic_replace`]。
    fn save_with_replace(
        &self,
        workspace_id: Uuid,
        state: &WorkspaceState,
        replace: impl FnOnce(&Path, &Path) -> Result<()>,
    ) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let target = self.path(workspace_id);
        let temporary = self
            .root
            .join(format!(".{workspace_id}.{}.tmp", Uuid::new_v4()));
        fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;

        if let Err(error) = replace(&temporary, &target) {
            match fs::remove_file(&temporary) {
                Ok(()) => {}
                Err(cleanup_error) if cleanup_error.kind() == std::io::ErrorKind::NotFound => {}
                Err(cleanup_error) => {
                    return Err(error.context(format!(
                        "发布工作区状态失败，且无法清理临时文件 {}：{cleanup_error}",
                        temporary.display()
                    )));
                }
            }
            return Err(error.context(format!("发布工作区状态失败：{}", target.display())));
        }

        Ok(())
    }

    fn path(&self, workspace_id: Uuid) -> PathBuf {
        self.root.join(format!("{workspace_id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_states_are_isolated_by_workspace_id() {
        let root = std::env::temp_dir().join(format!("apogee-workspace-state-{}", Uuid::new_v4()));
        let repository = WorkspaceStateRepository::new(&root);
        let first_workspace = Uuid::new_v4();
        let second_workspace = Uuid::new_v4();
        let environment_id = Uuid::new_v4();

        repository
            .save(
                first_workspace,
                &WorkspaceState {
                    active_environment_id: Some(environment_id),
                    active_environment_set: true,
                },
            )
            .unwrap();

        assert_eq!(
            repository
                .load(first_workspace)
                .unwrap()
                .active_environment_id,
            Some(environment_id)
        );
        assert_eq!(
            repository
                .load(second_workspace)
                .unwrap()
                .active_environment_id,
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_publish_preserves_existing_state_and_cleans_temporary_file() {
        let root = std::env::temp_dir().join(format!("apogee-workspace-state-{}", Uuid::new_v4()));
        let repository = WorkspaceStateRepository::new(&root);
        let workspace_id = Uuid::new_v4();
        let original = WorkspaceState {
            active_environment_id: Some(Uuid::new_v4()),
            active_environment_set: true,
        };
        repository.save(workspace_id, &original).unwrap();
        let persisted_before = fs::read(repository.path(workspace_id)).unwrap();

        let error = repository
            .save_with_replace(
                workspace_id,
                &WorkspaceState {
                    active_environment_id: None,
                    active_environment_set: false,
                },
                |_, _| anyhow::bail!("injected replacement failure"),
            )
            .unwrap_err();

        assert!(format!("{error:#}").contains("injected replacement failure"));
        assert_eq!(
            fs::read(repository.path(workspace_id)).unwrap(),
            persisted_before
        );
        let restored = repository.load(workspace_id).unwrap();
        assert_eq!(
            restored.active_environment_id,
            original.active_environment_id
        );
        assert_eq!(
            restored.active_environment_set,
            original.active_environment_set
        );
        assert!(fs::read_dir(root.join("workspace-state"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_workspace_state_is_quarantined_instead_of_blocking_open() {
        let root = std::env::temp_dir().join(format!("apogee-workspace-state-{}", Uuid::new_v4()));
        let repository = WorkspaceStateRepository::new(&root);
        let workspace_id = Uuid::new_v4();
        fs::create_dir_all(root.join("workspace-state")).unwrap();
        fs::write(repository.path(workspace_id), b"not-json").unwrap();

        let state = repository.load(workspace_id).unwrap();

        assert!(!state.active_environment_set);
        assert!(!repository.path(workspace_id).exists());
        assert_eq!(
            fs::read_dir(root.join("workspace-state")).unwrap().count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }
}
