use super::*;
use serde_json::json;

fn fixture() -> (PathBuf, WorkspaceRepository, WorkspaceRequest) {
    let root = std::env::temp_dir().join(format!("apogee-auth-storage-{}", Uuid::new_v4()));
    let repo = WorkspaceRepository::create(&root, "Auth storage".into()).unwrap();
    let parent = repo.snapshot().unwrap().nodes[0].id;
    let request = repo.create_request(parent, "Request".into()).unwrap();
    (root, repo, request)
}

#[test]
fn every_auth_type_roundtrips_without_literal_credentials_on_disk_or_export() {
    let (root, repo, mut request) = fixture();
    let marker = "synthetic-credential-认证";
    for value in [
        json!({"type":"basic","username":marker,"password":marker}),
        json!({"type":"bearer","token":marker.repeat(400)}),
        json!({"type":"oauth1","consumerKey":marker,"consumerSecret":marker,"token":marker,"tokenSecret":marker,"verifier":marker}),
        json!({"type":"oauth2","accessToken":marker,"tokenContext":marker,"tokenRequest":{"tokenUrl":"https://example.test/token","clientId":marker,"clientSecret":marker,"refreshToken":marker}}),
        json!({"type":"jwt","secret":marker,"payload":marker}),
        json!({"type":"apiKey","key":"X-Key","value":marker,"location":"header"}),
    ] {
        request.spec.auth = serde_json::from_value(value).unwrap();
        repo.save_request(request.id, request.name.clone(), request.spec.clone())
            .unwrap();
        let yaml = fs::read_to_string(repo.locate(request.id).unwrap().path).unwrap();
        assert!(!yaml.contains(marker));
        assert!(yaml.contains("authSecretRefs"));
        assert_eq!(
            serde_json::to_value(repo.read_request(request.id).unwrap().spec.auth).unwrap(),
            serde_json::to_value(&request.spec.auth).unwrap()
        );
        let exported = repo.export_postman(request.id).unwrap();
        assert!(!exported.json.contains(marker));
        assert!(!exported.warnings.is_empty());
    }
    repo.delete_node(request.id).unwrap();
    assert!(
        crate::secrets::test_auth_references(repo.id().unwrap(), request.id)
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cases_inheritance_copy_rename_rotation_and_delete_preserve_credentials() {
    let (root, repo, mut request) = fixture();
    let parent = repo.locate(request.id).unwrap().node.parent_id.unwrap();
    let mut container = repo.read_container(parent).unwrap();
    container.auth = AuthConfig::Bearer {
        token: "synthetic-parent".into(),
    };
    repo.save_container(container).unwrap();
    assert!(
        matches!(repo.resolve_request_inheritance(request.id, request.spec.clone()).unwrap().auth,
                     AuthConfig::Bearer {token} if token == "synthetic-parent")
    );
    request.spec.auth = AuthConfig::Bearer {
        token: "synthetic-case".into(),
    };
    let case = repo
        .create_debug_case(
            request.id,
            "Case".into(),
            Default::default(),
            request.spec.clone(),
        )
        .unwrap();
    repo.rename_node(request.id, "Renamed".into()).unwrap();
    repo.duplicate_node(request.id).unwrap();
    let copy = repo
        .snapshot()
        .unwrap()
        .nodes
        .into_iter()
        .find(|n| n.name == "Renamed 副本")
        .unwrap();
    let copied = repo.list_debug_cases(copy.id).unwrap();
    assert_ne!(copied[0].id, case.id);
    assert!(
        matches!(&copied[0].spec.auth, AuthConfig::Bearer {token} if token == "synthetic-case")
    );
    repo.delete_debug_case(request.id, case.id).unwrap();
    repo.compare_and_set_auth(
        parent,
        &AuthConfig::Bearer {
            token: "synthetic-parent".into(),
        },
        AuthConfig::Bearer {
            token: "synthetic-rotated".into(),
        },
    )
    .unwrap();
    repo.delete_node(request.id).unwrap();
    assert!(!repo.list_debug_cases(copy.id).unwrap().is_empty());
    repo.delete_node(parent).unwrap();
    for owner in [parent, request.id, copy.id] {
        assert!(
            crate::secrets::test_auth_references(repo.id().unwrap(), owner)
                .unwrap()
                .is_empty()
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_publication_keeps_old_generation_and_discards_new_credentials() {
    let (root, repo, mut request) = fixture();
    request.spec.auth = AuthConfig::Bearer {
        token: "synthetic-old".into(),
    };
    repo.save_request(request.id, request.name.clone(), request.spec.clone())
        .unwrap();
    let path = repo.locate(request.id).unwrap().path;
    let old = fs::read(&path).unwrap();
    let refs = crate::secrets::test_auth_references(repo.id().unwrap(), request.id).unwrap();
    let mut doc: PersistedRequest = repo.read_document(&path).unwrap();
    doc.spec.auth = AuthConfig::Bearer {
        token: "synthetic-new".into(),
    };
    assert!(repo
        .write_document_with(&path, &doc, |_, _| bail!("injected publish failure"))
        .is_err());
    assert_eq!(fs::read(&path).unwrap(), old);
    assert_eq!(
        crate::secrets::test_auth_references(repo.id().unwrap(), request.id).unwrap(),
        refs
    );
    assert!(repo
        .write_document_with(&path, &doc, |path, stored| {
            write_yaml(path, stored)?;
            bail!("after publish")
        })
        .is_err());
    assert!(
        matches!(repo.read_request(request.id).unwrap().spec.auth, AuthConfig::Bearer {token} if token == "synthetic-new")
    );
    repo.delete_node(request.id).unwrap();
    // Ambiguous post-publication errors deliberately retain the old generation.
    for reference in refs {
        crate::secrets::delete_auth(repo.id().unwrap(), request.id, reference).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_migration_templates_and_workspace_isolation_fail_closed() {
    let (root, repo, mut request) = fixture();
    let path = repo.locate(request.id).unwrap().path;
    request.spec.auth = AuthConfig::Basic {
        username: "{{client}}".into(),
        password: "synthetic-legacy".into(),
    };
    write_yaml(
        &path,
        &PersistedRequest {
            apogee: WORKSPACE_FORMAT_VERSION,
            id: request.id,
            name: request.name,
            spec: request.spec,
            debug_cases: vec![],
        },
    )
    .unwrap();
    repo.migrate_auth_credentials().unwrap();
    let yaml = fs::read_to_string(&path).unwrap();
    assert!(!yaml.contains("synthetic-legacy"));
    assert!(yaml.contains("{{client}}"));
    assert!(repo
        .export_postman(request.id)
        .unwrap()
        .json
        .contains("{{client}}"));
    let mut manifest = repo.read_manifest().unwrap();
    let original = manifest.id;
    manifest.id = Uuid::new_v4();
    write_yaml(&repo.manifest_path(), &manifest).unwrap();
    assert!(repo.snapshot().is_ok());
    assert!(repo.read_request(request.id).is_err());
    manifest.id = original;
    write_yaml(&repo.manifest_path(), &manifest).unwrap();
    assert!(
        matches!(repo.read_request(request.id).unwrap().spec.auth, AuthConfig::Basic {password,..} if password == "synthetic-legacy")
    );
    repo.delete_node(request.id).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_collection_copy_cleans_vault_but_keeps_source_credentials() {
    let (root, repo, mut request) = fixture();
    request.spec.auth = AuthConfig::Bearer {
        token: "synthetic-copy-source".into(),
    };
    repo.save_request(request.id, request.name.clone(), request.spec.clone())
        .unwrap();
    let collection = repo.locate(request.id).unwrap().node.parent_id.unwrap();
    let mut staged_refs = Vec::new();
    assert!(repo
        .duplicate_node_with_commit(collection, |_, manifest| {
            let copy = manifest.collections.last().unwrap();
            staged_refs = repo.auth_references(&root.join(&copy.directory))?;
            bail!("injected copy manifest failure")
        })
        .is_err());
    assert!(!staged_refs.is_empty());
    for (owner, _) in staged_refs {
        assert!(
            crate::secrets::test_auth_references(repo.id().unwrap(), owner)
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        matches!(repo.read_request(request.id).unwrap().spec.auth, AuthConfig::Bearer {token} if token == "synthetic-copy-source")
    );
    repo.delete_node(collection).unwrap();
    fs::remove_dir_all(root).unwrap();
}
