//! Private Auth persistence boundary. Executable/IPC models never contain vault references.
//! Only pure variable references remain portable; literal credential values live in the vault.
use super::*;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredDocument {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    auth_secret_refs: Vec<Uuid>,
    #[serde(flatten)]
    document: BTreeMap<String, Value>,
}

fn portable(value: &str) -> bool {
    value
        .strip_prefix("{{")
        .and_then(|v| v.strip_suffix("}}"))
        .is_some_and(|key| !key.trim().is_empty() && !key.contains(['{', '}']))
}

fn secret_paths(document: &Value) -> Vec<String> {
    let mut roots = vec!["/auth".to_string(), "/spec/auth".to_string()];
    if let Some(cases) = document.get("debugCases").and_then(Value::as_array) {
        roots.extend((0..cases.len()).map(|i| format!("/debugCases/{i}/spec/auth")));
    }
    let mut paths = Vec::new();
    for root in roots {
        let Some(auth) = document.pointer(&root) else {
            continue;
        };
        let fields: &[&str] = match auth.get("type").and_then(Value::as_str) {
            Some("basic") => &["username", "password"],
            Some("bearer") => &["token"],
            Some("oauth1") => &[
                "consumerKey",
                "consumerSecret",
                "token",
                "tokenSecret",
                "verifier",
            ],
            Some("oauth2") => &[
                "accessToken",
                "tokenContext",
                "tokenRequest/clientId",
                "tokenRequest/clientSecret",
                "tokenRequest/refreshToken",
            ],
            Some("jwt") => &["secret", "payload"],
            Some("apiKey") => &["value"],
            _ => &[],
        };
        paths.extend(fields.iter().map(|field| format!("{root}/{field}")));
    }
    paths
}

/// Also used for export of legacy files, without requiring access to the OS vault.
pub(super) fn strip_credentials(document: &mut Value) -> BTreeMap<String, String> {
    let mut secrets = BTreeMap::new();
    for path in secret_paths(document) {
        if let Some(value) = document.pointer_mut(&path) {
            if let Some(text) = value.as_str().filter(|v| !v.is_empty() && !portable(v)) {
                secrets.insert(path, text.to_owned());
                *value = Value::String(String::new());
            }
        }
    }
    secrets
}

impl WorkspaceRepository {
    pub(super) fn read_document<T: DeserializeOwned>(&self, path: &Path) -> Result<T> {
        let stored: StoredDocument = read_yaml(path)?;
        if stored.auth_secret_refs.is_empty() {
            return read_yaml(path);
        }
        let mut value = serde_json::to_value(&stored.document)?;
        let owner = document_id(&value)?;
        let workspace = self.id()?;
        let mut bundle = String::new();
        for reference in stored.auth_secret_refs {
            bundle.push_str(
                &crate::secrets::get_auth(workspace, owner, reference)?
                    .context("认证凭据缺失，请在当前工作区重新填写 Auth")?,
            );
        }
        let secrets: BTreeMap<String, String> =
            serde_json::from_str(&bundle).map_err(|_| anyhow::anyhow!("认证凭据存储格式无效"))?;
        let allowed = secret_paths(&value);
        for (path, secret) in secrets {
            if !allowed.contains(&path) {
                bail!("认证凭据引用无效");
            }
            let target = value.pointer_mut(&path).context("认证凭据字段缺失")?;
            // An explicit edit to a YAML field overrides the stored empty placeholder.
            if target.as_str() == Some("") {
                *target = Value::String(secret);
            }
        }
        serde_json::from_value(value).map_err(|_| anyhow::anyhow!("认证文档格式无效"))
    }

    pub(super) fn write_document<T: Serialize>(&self, path: &Path, document: &T) -> Result<()> {
        self.write_document_with(path, document, write_yaml)
    }

    fn write_document_with<T: Serialize>(
        &self,
        path: &Path,
        document: &T,
        publish: impl FnOnce(&Path, &StoredDocument) -> Result<()>,
    ) -> Result<()> {
        let mut value = serde_json::to_value(document)?;
        let secrets = strip_credentials(&mut value);
        let old: Option<StoredDocument> = path.is_file().then(|| read_yaml(path)).transpose()?;
        if secrets.is_empty() && old.as_ref().is_none_or(|v| v.auth_secret_refs.is_empty()) {
            return write_yaml(path, document);
        }
        let owner = document_id(&value)?;
        let workspace = self.id()?;
        let mut stored = StoredDocument {
            auth_secret_refs: Vec::new(),
            document: serde_json::from_value(value)?,
        };
        if !secrets.is_empty() {
            // Windows vault entries have a small platform limit. Split UTF-8 at character
            // boundaries; 1000 bytes also fits when encoded as UTF-16 by the provider.
            let bundle = serde_json::to_string(&secrets)?;
            let mut rest = bundle.as_str();
            while !rest.is_empty() {
                let mut end = rest.len().min(1000);
                while !rest.is_char_boundary(end) {
                    end -= 1;
                }
                let reference = Uuid::new_v4();
                if let Err(error) =
                    crate::secrets::set_auth(workspace, owner, reference, &rest[..end])
                {
                    delete_refs(workspace, owner, &stored.auth_secret_refs);
                    return Err(error);
                }
                stored.auth_secret_refs.push(reference);
                rest = &rest[end..];
            }
        }
        if let Err(error) = publish(path, &stored) {
            // Writers can fail after publication. Never remove credentials referenced by
            // the actual file; retain both generations when publication is uncertain.
            let published = read_yaml::<StoredDocument>(path)
                .map(|v| v.auth_secret_refs == stored.auth_secret_refs);
            if matches!(published, Ok(false)) || !path.exists() {
                delete_refs(workspace, owner, &stored.auth_secret_refs);
            }
            return Err(error);
        }
        if let Some(old) = old {
            let old_owner = document_id(&serde_json::to_value(&old.document)?)?;
            delete_refs(workspace, old_owner, &old.auth_secret_refs);
        }
        Ok(())
    }

    /// Called only under the workspace write lock, after transaction recovery.
    pub(crate) fn migrate_auth_credentials(&self) -> Result<()> {
        for item in self.scan()? {
            let path = if item.node.is_folder {
                item.path.join(container_marker(&item.node.kind)?)
            } else {
                item.path
            };
            let mut value: Value = read_yaml(&path)?;
            if !strip_credentials(&mut value).is_empty() {
                let value: Value = self.read_document(&path)?;
                self.write_document(&path, &value)?;
            }
        }
        Ok(())
    }

    /// Capture references before deletion; metadata reads never require a working vault.
    pub(super) fn auth_references(path: &Path) -> Result<Vec<(Uuid, Vec<Uuid>)>> {
        let mut refs = Vec::new();
        if !path.exists() {
            return Ok(refs);
        }
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            bail!("认证清理路径不能是符号链接");
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path)? {
                refs.extend(Self::auth_references(&entry?.path())?);
            }
        } else if path.extension().is_some_and(|ext| ext == "yaml") {
            let stored: StoredDocument = read_yaml(path)?;
            if !stored.auth_secret_refs.is_empty() {
                refs.push((
                    document_id(&serde_json::to_value(stored.document)?)?,
                    stored.auth_secret_refs,
                ));
            }
        }
        Ok(refs)
    }

    pub(super) fn delete_auth_references(&self, refs: Vec<(Uuid, Vec<Uuid>)>) {
        if let Ok(workspace) = self.id() {
            for (owner, refs) in refs {
                delete_refs(workspace, owner, &refs);
            }
        }
    }
}

fn document_id(value: &Value) -> Result<Uuid> {
    value
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
        .context("认证文档 ID 无效")
}

fn delete_refs(workspace: Uuid, owner: Uuid, refs: &[Uuid]) {
    for reference in refs {
        let _ = crate::secrets::delete_auth(workspace, owner, *reference);
    }
}

#[cfg(test)]
mod tests;
