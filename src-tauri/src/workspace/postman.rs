use super::*;
use apogee_core::postman::{export, PostmanImportResult};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostmanWorkspaceImportResult {
    pub collection_id: Uuid,
    pub imported_requests: usize,
    pub imported_folders: usize,
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
pub struct PostmanExportResult {
    pub json: String,
    pub warnings: Vec<String>,
}

fn export_scope_variables(variables: &[apogee_core::vars::EnvironmentVariable]) -> Vec<Value> {
    variables
        .iter()
        .map(|variable| {
            json!({
                "key": variable.key,
                "value": variable.value,
                "type": "string",
                "disabled": !variable.enabled,
            })
        })
        .collect()
}

impl WorkspaceRepository {
    pub fn export_postman(&self, id: Uuid) -> Result<PostmanExportResult> {
        let nodes = self.scan()?;
        let source = nodes
            .iter()
            .find(|item| item.node.id == id)
            .context("找不到导出来源")?;
        // Scan order is parents before children. Resolve each container once, including
        // ancestors outside a subfolder export; no per-request scans or IPC fan-out.
        let mut contexts = HashMap::<Uuid, (AuthConfig, RequestSettings)>::new();
        let mut children = HashMap::<Uuid, Vec<Value>>::new();
        let mut items = HashMap::<Uuid, Value>::new();
        let mut warnings = vec![
            "安全导出不包含 Auth 中直接填写的凭据，仅保留变量引用；请按需在目标客户端配置凭据"
                .to_string(),
        ];
        for item in &nodes {
            let (mut auth, mut settings) = item
                .node
                .parent_id
                .and_then(|id| contexts.get(&id))
                .cloned()
                .unwrap_or_default();
            let included = item.path == source.path
                || (source.node.is_folder && item.path.starts_with(&source.path));
            if item.node.is_folder {
                let document: ContainerDocument =
                    read_yaml(&item.path.join(container_marker(&item.node.kind)?))?;
                if !matches!(document.auth, AuthConfig::Inherit) {
                    auth = document.auth;
                }
                merge_settings(&mut settings, &document.settings);
                contexts.insert(item.node.id, (auth, settings));
                if included {
                    items.insert(
                        item.node.id,
                        json!({
                            "name": item.node.name,
                            "item": [],
                            "variable": export_scope_variables(&document.variables),
                        }),
                    );
                }
            } else if included {
                let document: PersistedRequest = read_yaml(&item.path)?;
                let mut spec = document.spec;
                if matches!(spec.auth, AuthConfig::Inherit) {
                    spec.auth = auth;
                }
                merge_settings(&mut settings, &spec.settings);
                spec.settings = settings;
                let mut safe = serde_json::to_value(&spec)?;
                auth_storage::strip_credentials(&mut safe);
                spec = serde_json::from_value(safe)?;
                if serde_json::to_value(&spec.settings)?
                    .as_object()
                    .unwrap()
                    .values()
                    .any(|v| !v.is_null())
                    || matches!(
                        spec.auth,
                        AuthConfig::OAuth2 {
                            auto_refresh: true,
                            ..
                        }
                    )
                {
                    warnings.push(format!("请求「{}」：网络设置、Cookie 会话和自动刷新行为无法完整表示为 Postman Collection，请在目标客户端检查",item.node.name));
                }
                items.insert(
                    item.node.id,
                    json!({"name":item.node.name,"request":export::request(&spec)}),
                );
            }
        }
        for item in nodes.iter().rev() {
            let Some(mut value) = items.remove(&item.node.id) else {
                continue;
            };
            if item.node.is_folder {
                value["item"] = json!(children
                    .remove(&item.node.id)
                    .unwrap_or_default()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>());
            }
            if item.node.id == id {
                items.insert(id, value);
                break;
            }
            if let Some(parent) = item.node.parent_id {
                children.entry(parent).or_default().push(value);
            }
        }
        let root = items.remove(&id).context("导出范围无效")?;
        let values = if matches!(source.node.kind, WorkspaceNodeKind::Collection) {
            root["item"].clone()
        } else {
            json!([root])
        };
        let mut payload = json!({"info":{"name":source.node.name,"schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":values});
        if matches!(source.node.kind, WorkspaceNodeKind::Collection) {
            payload["variable"] = root["variable"].clone();
        }
        Ok(PostmanExportResult {
            json: serde_json::to_string_pretty(&payload)?,
            warnings,
        })
    }

    pub fn import_postman(
        &self,
        plan: &PostmanImportResult,
    ) -> Result<PostmanWorkspaceImportResult> {
        self.import_postman_with(plan, |_| Ok(()), write_yaml)
    }
    fn import_postman_with(
        &self,
        plan: &PostmanImportResult,
        mut stage_write: impl FnMut(&Path) -> Result<()>,
        commit: impl FnOnce(&Path, &WorkspaceManifest) -> Result<()>,
    ) -> Result<PostmanWorkspaceImportResult> {
        if plan.requests.is_empty() {
            bail!("Postman 集合中没有可导入的请求");
        }
        super::validate_variable_names(&plan.variables)?;
        let name = required_name(&plan.name)?;
        // Validate the complete identity graph and every name before staging anything.
        let mut known = std::collections::HashSet::new();
        known.insert(Vec::<usize>::new());
        for folder in &plan.folders {
            required_name(&folder.name)?;
            super::validate_variable_names(&folder.variables)?;
            if folder.path.is_empty()
                || !known.contains(&folder.path[..folder.path.len() - 1])
                || !known.insert(folder.path.clone())
            {
                bail!("导入目录身份无效");
            }
        }
        for request in &plan.requests {
            required_name(&request.name)?;
            if !known.contains(&request.folder_ids) {
                bail!("导入请求目录不存在");
            }
        }
        let mut manifest = self.read_manifest()?;
        let id = Uuid::new_v4();
        let journal = self.begin_transaction(WorkspaceTransactionOperation::ImportCollection {
            collection_id: id,
        })?;
        let staging = self.transaction_directory().join(format!("import-{id}"));
        let target = self.root.join("collections").join(id.to_string());
        let result = (|| -> Result<()> {
            fs::create_dir_all(&staging)?;
            self.write_document(
                &staging.join(COLLECTION_MARKER),
                &ContainerDocument {
                    apogee: WORKSPACE_FORMAT_VERSION,
                    id,
                    name: name.clone(),
                    auth: AuthConfig::Inherit,
                    settings: RequestSettings::default(),
                    variables: plan.variables.clone(),
                },
            )?;
            let mut paths = HashMap::new();
            paths.insert(Vec::<usize>::new(), staging.clone());
            for folder in &plan.folders {
                let parent = paths
                    .get(&folder.path[..folder.path.len() - 1])
                    .context("导入目录顺序无效")?;
                let id = Uuid::new_v4();
                let path = parent.join(id.to_string());
                fs::create_dir_all(&path)?;
                self.write_document(
                    &path.join(FOLDER_MARKER),
                    &ContainerDocument {
                        apogee: WORKSPACE_FORMAT_VERSION,
                        id,
                        name: required_name(&folder.name)?,
                        auth: AuthConfig::Inherit,
                        settings: RequestSettings::default(),
                        variables: folder.variables.clone(),
                    },
                )?;
                paths.insert(folder.path.clone(), path);
            }
            for request in &plan.requests {
                let parent = paths.get(&request.folder_ids).context("导入请求目录无效")?;
                let id = Uuid::new_v4();
                stage_write(parent)?;
                self.write_document(
                    &parent.join(format!("{id}.yaml")),
                    &PersistedRequest {
                        apogee: WORKSPACE_FORMAT_VERSION,
                        id,
                        name: required_name(&request.name)?,
                        spec: request.spec.clone(),
                        debug_cases: Vec::new(),
                    },
                )?;
            }
            validate_import_tree(&staging)?;

            fs::rename(&staging, &target)
                .context("发布导入集合失败，请检查目录占用和写入权限后重试")?;
            manifest.collections.push(CollectionEntry {
                id,
                name,
                directory: format!("collections/{id}"),
            });
            commit(&self.manifest_path(), &manifest)
        })();
        let committed = self
            .read_manifest()?
            .collections
            .iter()
            .any(|entry| entry.id == id);
        self.recover_postman_import(&self.read_manifest()?, id, &journal)?;
        if !committed {
            result?;
            bail!("导入集合未提交");
        }
        // Manifest publication is the commit point, even if a writer reports a later error.
        Ok(PostmanWorkspaceImportResult {
            collection_id: id,
            imported_requests: plan.requests.len(),
            imported_folders: plan.folders.len(),
            warnings: plan.warnings.clone(),
        })
    }

    pub(super) fn recover_postman_import(
        &self,
        manifest: &WorkspaceManifest,
        id: Uuid,
        journal: &Path,
    ) -> Result<()> {
        let staging = self.transaction_directory().join(format!("import-{id}"));
        let refs = self.auth_references(&staging)?;
        remove_directory_if_exists(&staging)?;
        self.delete_auth_references(refs);
        let target = self.root.join("collections").join(id.to_string());
        if target.exists() && !manifest.collections.iter().any(|entry| entry.id == id) {
            let marker: ContainerDocument = read_yaml(&target.join(COLLECTION_MARKER))?;
            if marker.id != id || fs::symlink_metadata(&target)?.file_type().is_symlink() {
                bail!("导入事务的集合标识或目录无效");
            }
            let refs = self.auth_references(&target)?;
            fs::remove_dir_all(target)?;
            self.delete_auth_references(refs);
        }
        self.finish_transaction(journal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, WorkspaceRepository, PostmanImportResult) {
        let root = std::env::temp_dir().join(format!("apogee-postman-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create(&root, "fixture".into()).unwrap();
        let plan = apogee_core::postman::import_json(&json!({"info":{"name":"fixture"},"auth":{"type":"bearer","bearer":{"token":"fixture-token"}},"item":[
            {"name":"A/B","item":[{"name":"slash","request":{"url":"https://example.test/"}}]},
            {"name":"A","item":[{"name":"B","item":[{"name":"nested","request":{"url":"https://example.test/"}}]}]},
            {"name":"Same","item":[{"name":"first","request":{"url":"https://example.test/"}}]},
            {"name":"Same","item":[{"name":"second","request":{"url":"https://example.test/","auth":{"type":"noauth"}}}]},
            {"name":" spaced ","item":[]}
        ]}).to_string()).unwrap();
        (root, repository, plan)
    }
    #[test]
    fn identity_graph_preserves_slashes_same_named_siblings_and_empty_folders() {
        let (root, repository, plan) = fixture();
        let result = repository.import_postman(&plan).unwrap();
        let snapshot = repository.snapshot().unwrap();
        assert_eq!(result.imported_folders, 6);
        let parent = |name| {
            snapshot
                .nodes
                .iter()
                .find(|node| node.name == name)
                .unwrap()
                .parent_id
                .unwrap()
        };
        assert_ne!(parent("slash"), parent("nested"));
        assert_ne!(parent("first"), parent("second"));
        assert!(snapshot.nodes.iter().any(|node| node.name == "spaced"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn validation_staging_and_manifest_failures_leave_no_partial_collection() {
        let (root, repository, mut plan) = fixture();
        let before = fs::read(repository.manifest_path()).unwrap();
        let snapshot_before = serde_json::to_value(repository.snapshot().unwrap()).unwrap();
        let collection_count = repository.read_manifest().unwrap().collections.len();
        plan.requests[1].name = "x".repeat(121);
        assert!(repository.import_postman(&plan).is_err());
        plan.requests[1].name = "valid".into();
        let mut staged = 0;
        assert!(repository
            .import_postman_with(
                &plan,
                |_| {
                    staged += 1;
                    if staged == 2 {
                        bail!("injected second request write failure");
                    }
                    Ok(())
                },
                write_yaml
            )
            .is_err());
        assert_eq!(staged, 2);
        assert!(repository
            .import_postman_with(&plan, |_| Ok(()), |_, _| bail!("injected manifest failure"))
            .is_err());
        assert_eq!(fs::read(repository.manifest_path()).unwrap(), before);
        assert_eq!(
            serde_json::to_value(repository.snapshot().unwrap()).unwrap(),
            snapshot_before
        );
        assert_eq!(
            fs::read_dir(root.join("collections")).unwrap().count(),
            collection_count
        );
        repository.import_postman(&plan).unwrap();
        assert_eq!(
            repository.read_manifest().unwrap().collections.len(),
            collection_count + 1
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn recovery_removes_orphan_import_but_keeps_committed_import() {
        let (root, repository, plan) = fixture();
        let id = Uuid::new_v4();
        let journal = repository
            .begin_transaction(WorkspaceTransactionOperation::ImportCollection {
                collection_id: id,
            })
            .unwrap();
        let target = root.join("collections").join(id.to_string());
        fs::create_dir_all(&target).unwrap();
        write_yaml(
            &target.join(COLLECTION_MARKER),
            &ContainerDocument {
                apogee: WORKSPACE_FORMAT_VERSION,
                id,
                name: "orphan".into(),
                auth: AuthConfig::Inherit,
                settings: RequestSettings::default(),
                variables: Vec::new(),
            },
        )
        .unwrap();
        fs::create_dir_all(
            repository
                .transaction_directory()
                .join(format!("import-{id}")),
        )
        .unwrap();
        repository.recover_pending_transactions().unwrap();
        assert!(!target.exists());
        assert!(!journal.exists());
        let result = repository
            .import_postman_with(
                &plan,
                |_| Ok(()),
                |path, manifest| {
                    write_yaml(path, manifest)?;
                    bail!("failure after commit")
                },
            )
            .unwrap();
        let journal = repository
            .begin_transaction(WorkspaceTransactionOperation::ImportCollection {
                collection_id: result.collection_id,
            })
            .unwrap();
        repository.recover_pending_transactions().unwrap();
        assert!(!journal.exists());
        assert_eq!(
            repository
                .snapshot()
                .unwrap()
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, WorkspaceNodeKind::Request))
                .count(),
            4
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn exports_effective_auth_for_collection_subfolder_and_single_request() {
        let (root, repository, plan) = fixture();
        let imported = repository.import_postman(&plan).unwrap();
        let snapshot = repository.snapshot().unwrap();
        let first = snapshot
            .nodes
            .iter()
            .find(|node| node.name == "first")
            .unwrap();
        // Restore inheritance on disk, so this exercises repository resolution on export.
        let mut request = repository.read_request(first.id).unwrap();
        request.spec.auth = AuthConfig::Inherit;
        repository
            .save_request(first.id, request.name, request.spec)
            .unwrap();
        let mut container = repository.read_container(imported.collection_id).unwrap();
        container.auth = AuthConfig::Bearer {
            token: "fixture-token".into(),
        };
        container.settings.verify_tls = Some(false);
        repository.save_container(container.clone()).unwrap();
        for id in [imported.collection_id, first.parent_id.unwrap(), first.id] {
            let result = repository.export_postman(id).unwrap();
            assert!(!result.warnings.is_empty());
            let roundtrip = apogee_core::postman::import_json(&result.json).unwrap();
            let first = roundtrip
                .requests
                .iter()
                .find(|request| request.name == "first")
                .unwrap();
            assert!(matches!(&first.spec.auth,AuthConfig::Bearer {token} if token.is_empty()));
            if id == imported.collection_id {
                assert!(matches!(
                    roundtrip
                        .requests
                        .iter()
                        .find(|request| request.name == "second")
                        .unwrap()
                        .spec
                        .auth,
                    AuthConfig::None
                ));
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn postman_scope_variables_roundtrip_through_collection_export() {
        let root = std::env::temp_dir().join(format!("apogee-postman-scopes-{}", Uuid::new_v4()));
        let repository = WorkspaceRepository::create(&root, "scopes".into()).unwrap();
        let plan = apogee_core::postman::import_json(
            &json!({
                "info": {"name": "scope-fixture"},
                "variable": [{"key":"collectionValue","value":"collection-data"}],
                "item": [{
                    "name":"Folder",
                    "variable":[{"key":"folderValue","value":"folder-data"}],
                    "item":[{"name":"Request","request":{"url":"https://example.test"}}]
                }]
            })
            .to_string(),
        )
        .unwrap();
        let imported = repository.import_postman(&plan).unwrap();
        let exported = repository.export_postman(imported.collection_id).unwrap();
        let json: Value = serde_json::from_str(&exported.json).unwrap();
        assert_eq!(json["variable"][0]["key"], "collectionValue");
        assert_eq!(json["variable"][0]["value"], "collection-data");
        assert_eq!(json["item"][0]["variable"][0]["key"], "folderValue");
        assert_eq!(json["item"][0]["variable"][0]["value"], "folder-data");

        let roundtrip = apogee_core::postman::import_json(&exported.json).unwrap();
        assert_eq!(roundtrip.variables[0].key, "collectionValue");
        assert_eq!(roundtrip.folders[0].variables[0].key, "folderValue");
        fs::remove_dir_all(root).unwrap();
    }
}
