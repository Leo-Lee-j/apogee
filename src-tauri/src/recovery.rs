//! Bounded, machine-local editor recovery. Never stored in the shareable workspace.
use crate::persistence::atomic_write;
use anyhow::{bail, Context, Result};
use apogee_core::{
    request::{AuthConfig, RequestSpec},
    workspace::WorkspaceContainer,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DRAFTS: usize = 100;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDraft {
    pub id: Uuid,
    pub title: String,
    pub spec: RequestSpec,
    pub node_id: Option<Uuid>,
    pub expected_auth: Option<AuthConfig>,
    pub source_request_id: Option<Uuid>,
    pub debug_case_id: Option<Uuid>,
    #[serde(default)]
    pub debug_case_editor: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerDraft {
    pub value: WorkspaceContainer,
    pub expected_auth: AuthConfig,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverySnapshot {
    #[serde(default)]
    pub requests: Vec<RequestDraft>,
    #[serde(default)]
    pub containers: Vec<ContainerDraft>,
}

impl RecoverySnapshot {
    fn validate(&self) -> Result<()> {
        if self.requests.len() + self.containers.len() > MAX_DRAFTS {
            bail!("最多保留 100 项恢复草稿，请先保存或关闭部分编辑");
        }
        let mut ids = HashSet::new();
        for request in &self.requests {
            if !ids.insert(request.id) || request.title.len() > 1024 {
                bail!("恢复草稿的标识或名称无效");
            }
        }
        ids.clear();
        for container in &self.containers {
            if !ids.insert(container.value.id) {
                bail!("恢复草稿的容器标识重复");
            }
        }
        Ok(())
    }
}

pub struct RecoveryRepository {
    root: PathBuf,
}
impl RecoveryRepository {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("editor-recovery"),
        }
    }
    fn path(&self, workspace: Uuid) -> PathBuf {
        self.root.join(format!("{workspace}.json"))
    }
    pub fn load(&self, workspace: Uuid) -> Result<RecoverySnapshot> {
        let path = self.path(workspace);
        if !path.exists() {
            return Ok(RecoverySnapshot::default());
        }
        if fs::metadata(&path)?.len() > MAX_BYTES {
            bail!("恢复草稿超过 8 MiB，请先备份或移走本机恢复文件");
        }
        let data = fs::read(&path)?;
        let snapshot: RecoverySnapshot = serde_json::from_slice(&data)
            .map_err(|_| anyhow::anyhow!("本机恢复草稿损坏，请备份恢复文件后重试"))?;
        snapshot.validate()?;
        Ok(snapshot)
    }
    pub fn save(&self, workspace: Uuid, snapshot: &RecoverySnapshot) -> Result<()> {
        snapshot.validate()?;
        let data = serde_json::to_vec(snapshot).map_err(|_| anyhow::anyhow!("无法编码恢复草稿"))?;
        if data.len() as u64 > MAX_BYTES {
            bail!("恢复草稿超过 8 MiB，请将大请求保存到集合后重试");
        }
        atomic_write(&self.path(workspace), &data).context("保存本机恢复草稿失败")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft() -> RequestDraft {
        RequestDraft {
        id: Uuid::new_v4(), title: "draft".into(),
        spec: serde_json::from_value(serde_json::json!({"method":"POST","url":"https://example.test","body":{"type":"json","content":"private fixture"}})).unwrap(),
        node_id: None, expected_auth: None, source_request_id: None, debug_case_id: None, debug_case_editor: false,
    }
    }
    #[test]
    fn recovery_is_isolated_and_rejected_writes_preserve_previous_content() {
        let root = std::env::temp_dir().join(format!("apogee-recovery-{}", Uuid::new_v4()));
        let repository = RecoveryRepository::new(&root);
        let workspace = Uuid::new_v4();
        let mut snapshot = RecoverySnapshot {
            requests: vec![draft()],
            containers: vec![],
        };
        repository.save(workspace, &snapshot).unwrap();
        assert_eq!(repository.load(workspace).unwrap().requests.len(), 1);
        assert!(repository.load(Uuid::new_v4()).unwrap().requests.is_empty());
        let before = fs::read(repository.path(workspace)).unwrap();
        snapshot.requests.push(snapshot.requests[0].clone());
        assert!(repository.save(workspace, &snapshot).is_err());
        assert_eq!(fs::read(repository.path(workspace)).unwrap(), before);
        snapshot.requests = (0..101).map(|_| draft()).collect();
        assert!(repository.save(workspace, &snapshot).is_err());
        snapshot.requests = vec![draft()];
        snapshot.requests[0].title = "x".repeat(MAX_BYTES as usize);
        assert!(repository.save(workspace, &snapshot).is_err());
        assert_eq!(fs::read(repository.path(workspace)).unwrap(), before);
        snapshot.requests = vec![draft()];
        snapshot.requests[0].spec.body = apogee_core::request::Body::Json {
            content: "x".repeat(MAX_BYTES as usize),
        };
        assert!(repository.save(workspace, &snapshot).is_err());
        assert_eq!(fs::read(repository.path(workspace)).unwrap(), before);
        fs::write(repository.path(workspace), b"private fixture invalid JSON").unwrap();
        let error = repository.load(workspace).err().unwrap().to_string();
        assert!(!error.contains("private fixture"));
        fs::remove_dir_all(root).unwrap();
    }
}
