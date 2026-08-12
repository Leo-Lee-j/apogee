use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default)]
    pub settings_version: u32,
    pub default_workspace_root: String,
    #[serde(default)]
    pub open_last_workspace_on_start: bool,
    #[serde(default = "default_show_memory_monitor")]
    pub show_memory_monitor: bool,
    #[serde(default)]
    pub low_memory_mode: bool,
    #[serde(default = "default_proxy_mode")]
    pub proxy_mode: String,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_verify_tls")]
    pub verify_tls: bool,
    #[serde(default = "default_theme_mode")]
    pub theme_mode: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_font_family")]
    pub font_family: String,
}

fn default_show_memory_monitor() -> bool {
    true
}

fn default_proxy_mode() -> String { "system".to_string() }
fn default_timeout_ms() -> u64 { 30_000 }
fn default_verify_tls() -> bool { true }
fn default_theme_mode() -> String { "light".to_string() }
fn default_language() -> String { "system".to_string() }
fn default_font_family() -> String { "system".to_string() }

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
                settings_version: 1,
                default_workspace_root: self
                    .app_data_dir
                    .join("workspaces")
                    .to_string_lossy()
                    .into_owned(),
                open_last_workspace_on_start: false,
                show_memory_monitor: true,
                low_memory_mode: false,
                proxy_mode: default_proxy_mode(),
                proxy_url: None,
                timeout_ms: default_timeout_ms(),
                verify_tls: default_verify_tls(),
                theme_mode: default_theme_mode(),
                language: default_language(),
                font_family: default_font_family(),
            };
            self.save(&settings)?;
            return Ok(settings);
        }
        let mut settings: AppSettings = serde_json::from_slice(&fs::read(path)?)?;
        // Version 0 was written when "system" was the implicit theme default.
        // Migrate that implicit value once; an explicit choice made after this
        // version is preserved because saves carry settings_version = 1.
        let needs_theme_migration = settings.settings_version == 0 && settings.theme_mode == "system";
        if needs_theme_migration {
            settings.theme_mode = "light".to_string();
        }
        let original_root = settings.default_workspace_root.clone();
        settings.default_workspace_root = clean_windows_path(&settings.default_workspace_root);
        fs::create_dir_all(&settings.default_workspace_root)?;
        if settings.default_workspace_root != original_root || needs_theme_migration || settings.settings_version == 0 {
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
            settings_version: 1,
            default_workspace_root: clean_windows_path(&normalized.to_string_lossy()),
            open_last_workspace_on_start: settings.open_last_workspace_on_start,
            show_memory_monitor: settings.show_memory_monitor,
            low_memory_mode: settings.low_memory_mode,
            proxy_mode: settings.proxy_mode.clone(),
            proxy_url: settings.proxy_url.clone(),
            timeout_ms: settings.timeout_ms,
            verify_tls: settings.verify_tls,
            theme_mode: settings.theme_mode.clone(),
            language: settings.language.clone(),
            font_family: settings.font_family.clone(),
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
