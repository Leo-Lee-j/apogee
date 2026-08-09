use crate::model::KeyValue;
use crate::request::{AuthConfig, Body, RequestSpec};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedRequest {
    pub name: String,
    pub spec: RequestSpec,
    pub folder: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostmanImportResult {
    pub name: String,
    pub requests: Vec<ImportedRequest>,
    pub warnings: Vec<String>,
}

pub fn import_json(input: &str) -> Result<PostmanImportResult, crate::error::AppError> {
    let root: serde_json::Value = serde_json::from_str(input).map_err(|e| {
        crate::error::AppError::new(crate::error::ErrorKind::Parse, "Postman JSON 无效")
            .detail(e.to_string())
    })?;
    let name = root
        .pointer("/info/name")
        .and_then(|v| v.as_str())
        .unwrap_or("Imported Collection")
        .to_string();
    let mut requests = Vec::new();
    let mut warnings = Vec::new();
    walk_items(
        root.get("item").and_then(|v| v.as_array()),
        &mut Vec::new(),
        &mut requests,
        &mut warnings,
    );
    if requests.is_empty() {
        warnings.push("集合中没有可导入的请求".into());
    }
    Ok(PostmanImportResult {
        name,
        requests,
        warnings,
    })
}

fn walk_items(
    items: Option<&Vec<serde_json::Value>>,
    folder: &mut Vec<String>,
    out: &mut Vec<ImportedRequest>,
    warnings: &mut Vec<String>,
) {
    for item in items.into_iter().flatten() {
        if let Some(children) = item.get("item").and_then(|v| v.as_array()) {
            if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                folder.push(name.into());
            }
            walk_items(Some(children), folder, out, warnings);
            if item.get("name").is_some() {
                folder.pop();
            }
            continue;
        }
        let Some(request) = item.get("request") else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_string();
        let url = request
            .get("url")
            .and_then(|v| v.as_str())
            .or_else(|| {
                request
                    .get("url")
                    .and_then(|v| v.get("raw"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            warnings.push(format!(
                "请求 {} 缺少 URL",
                item.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未命名")
            ));
            continue;
        }
        let headers = rows(request.get("header"));
        let body = request
            .get("body")
            .and_then(|b| b.get("raw"))
            .and_then(|v| v.as_str())
            .map(|s| Body::Json { content: s.into() })
            .unwrap_or(Body::None);
        out.push(ImportedRequest {
            name: item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Imported Request")
                .into(),
            folder: folder.clone(),
            spec: RequestSpec {
                method,
                url,
                params: Vec::new(),
                path_params: Vec::new(),
                headers,
                body,
                auth: AuthConfig::None,
                settings: Default::default(),
            },
        });
    }
}
fn rows(value: Option<&serde_json::Value>) -> Vec<KeyValue> {
    value
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| {
            Some(KeyValue {
                enabled: !v.get("disabled").and_then(|x| x.as_bool()).unwrap_or(false),
                key: v.get("key")?.as_str()?.into(),
                value: v.get("value").and_then(|x| x.as_str()).unwrap_or("").into(),
                description: None,
                kind: crate::model::ValueKind::Text,
            })
        })
        .collect()
}
