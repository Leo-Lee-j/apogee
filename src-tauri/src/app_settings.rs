use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub default_workspace_root: String,
    #[serde(default)]
    pub open_last_workspace_on_start: bool,
}

pub struct AppSettingsRepository {
    app_data_dir: PathBuf,
}

impl AppSettingsRepository {
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            app_data_dir: app_data_dir.to_path_buf(),
        }
    }

    pub fn load(&self) -> Result<AppSettings> {
        let path = self.path();
        if !path.exists() {
            let settings = AppSettings {
                default_workspace_root: self
                    .app_data_dir
                    .join("workspaces")
                    .to_string_lossy()
                    .into_owned(),
                open_last_workspace_on_start: false,
            };
            self.save(&settings)?;
            return Ok(settings);
        }
        let mut settings: AppSettings = serde_json::from_slice(&fs::read(path)?)?;
        let original_root = settings.default_workspace_root.clone();
        settings.default_workspace_root = clean_windows_path(&settings.default_workspace_root);
        fs::create_dir_all(&settings.default_workspace_root)?;
        if settings.default_workspace_root != original_root {
            self.save(&settings)?;
        }
        Ok(settings)
    }

    pub fn save(&self, settings: &AppSettings) -> Result<()> {
        let root = PathBuf::from(settings.default_workspace_root.trim());
        if settings.default_workspace_root.trim().is_empty() {
            anyhow::bail!("默认工作区位置不能为空");
        }
        fs::create_dir_all(&root)
            .with_context(|| format!("无法创建默认工作区目录：{}", root.display()))?;
        let normalized = root
            .canonicalize()
            .with_context(|| format!("无法解析默认工作区目录：{}", root.display()))?;
        let normalized_settings = AppSettings {
            default_workspace_root: clean_windows_path(&normalized.to_string_lossy()),
            open_last_workspace_on_start: settings.open_last_workspace_on_start,
        };
        fs::create_dir_all(&self.app_data_dir)?;
        let temporary = self
            .app_data_dir
            .join(format!(".settings.{}.tmp", Uuid::new_v4()));
        fs::write(&temporary, serde_json::to_vec_pretty(&normalized_settings)?)?;
        let target = self.path();
        if target.exists() {
            fs::remove_file(&target)?;
        }
        fs::rename(temporary, target)?;
        Ok(())
    }

    fn path(&self) -> PathBuf {
        self.app_data_dir.join("settings.json")
    }
}

pub fn clean_windows_path(value: &str) -> String {
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        value.to_string()
    }
}
