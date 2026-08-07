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

    pub fn save(&self, workspace_id: Uuid, state: &WorkspaceState) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let target = self.path(workspace_id);
        let temporary = self
            .root
            .join(format!(".{workspace_id}.{}.tmp", Uuid::new_v4()));
        fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
        match fs::rename(&temporary, &target) {
            Ok(()) => Ok(()),
            Err(_rename_error) if target.exists() => {
                fs::remove_file(&target)
                    .with_context(|| format!("替换旧工作区状态失败：{}", target.display()))?;
                fs::rename(&temporary, &target)
                    .with_context(|| format!("发布工作区状态失败：{}", target.display()))?;
                Ok(())
            }
            Err(rename_error) => Err(rename_error.into()),
        }
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
