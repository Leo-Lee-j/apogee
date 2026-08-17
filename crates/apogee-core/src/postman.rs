use crate::model::KeyValue;
use crate::request::{AuthConfig, Body, RequestSpec};
use crate::vars::{Environment, EnvironmentVariable};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostmanEnvironmentImportResult {
    pub environment: Environment,
    pub warnings: Vec<String>,
}

/// Imports the portable Postman Environment export format into an Apogee environment.
///
/// Postman's `initial` and `current` values can differ. Exported collections use the
/// current value at runtime, so that is the only value we preserve. We intentionally do
/// not infer a `host`: any key may be used as a URL fragment and guessing would change it.
pub fn import_environment_json(
    input: &str,
) -> Result<PostmanEnvironmentImportResult, crate::error::AppError> {
    let root: serde_json::Value = serde_json::from_str(input).map_err(|error| {
        crate::error::AppError::new(
            crate::error::ErrorKind::Parse,
            "Postman Environment JSON 无效",
        )
        .detail(error.to_string())
    })?;
    let name = root
        .get("name")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Imported Environment")
        .to_string();
    let values = root
        .get("values")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            crate::error::AppError::new(
                crate::error::ErrorKind::Parse,
                "这不是 Postman Environment 导出文件",
            )
            .hint(crate::error::Hint::new(
                "请选择包含 values 数组的 Postman Environment JSON 文件",
            ))
        })?;
    let mut warnings = Vec::new();
    let mut variables = Vec::new();
    for value in values {
        let Some(key) = value.get("key").and_then(|item| item.as_str()) else {
            warnings.push("跳过了缺少 key 的环境变量".into());
            continue;
        };
        if key.trim().is_empty() {
            warnings.push("跳过了空名称的环境变量".into());
            continue;
        }
        let current = value
            .get("value")
            .or_else(|| value.get("current"))
            .and_then(|item| item.as_str())
            .unwrap_or_default();
        variables.push(EnvironmentVariable {
            key: key.to_string(),
            value: current.to_string(),
            enabled: value
                .get("enabled")
                .and_then(|item| item.as_bool())
                .unwrap_or(true),
        });
    }
    Ok(PostmanEnvironmentImportResult {
        environment: Environment {
            id: Uuid::new_v4(),
            name,
            host: String::new(),
            variables,
        },
        warnings,
    })
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
        let url_value = request.get("url");
        let url = url_value
            .and_then(|value| value.as_str())
            .or_else(|| {
                url_value
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
        let params = url_value
            .and_then(|value| value.get("query"))
            .map(rows_value)
            .unwrap_or_default();
        let path_params = url_value
            .and_then(|value| value.get("variable"))
            .map(rows_value)
            .unwrap_or_default();
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
                // Apifox exports its service URL as `{{baseUrl}}` in the Postman
                // collection, but not its value. Apogee represents that scope as the
                // active environment host, so keep only the relative request path.
                url: normalize_apifox_base_url(&url),
                params,
                path_params,
                headers,
                body,
                auth: AuthConfig::None,
                settings: Default::default(),
            },
        });
    }
}
fn rows(value: Option<&serde_json::Value>) -> Vec<KeyValue> {
    value.map(rows_value).unwrap_or_default()
}

fn rows_value(value: &serde_json::Value) -> Vec<KeyValue> {
    let values: Vec<&serde_json::Value> = match value {
        serde_json::Value::Array(values) => values.iter().collect(),
        serde_json::Value::Object(_) => vec![value],
        _ => Vec::new(),
    };
    values
        .into_iter()
        .filter_map(|v| {
            Some(KeyValue {
                enabled: !v.get("disabled").and_then(|x| x.as_bool()).unwrap_or(false),
                key: v.get("key")?.as_str()?.into(),
                value: value_text(v.get("value")).into(),
                description: v
                    .get("description")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                kind: crate::model::ValueKind::Text,
            })
        })
        .collect()
}

fn value_text(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(value) => value.to_string(),
    }
}

fn normalize_apifox_base_url(url: &str) -> String {
    let trimmed = url.trim();
    let Some(rest) = trimmed
        .strip_prefix("{{baseUrl}}")
        .or_else(|| trimmed.strip_prefix("{{baseURL}}"))
    else {
        return trimmed.to_string();
    };
    if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#') {
        rest.to_string()
    } else {
        format!("/{rest}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_postman_environment_current_values() {
        let imported = import_environment_json(
            r#"{
          "name": "Staging",
          "values": [
            {"key": "baseUrl", "value": "https://staging.example.test", "enabled": true},
            {"key": "token", "current": "abc", "enabled": false},
            {"value": "ignored"}
          ]
        }"#,
        )
        .unwrap();

        assert_eq!(imported.environment.name, "Staging");
        assert_eq!(imported.environment.variables.len(), 2);
        assert_eq!(
            imported.environment.variables[0].value,
            "https://staging.example.test"
        );
        assert!(!imported.environment.variables[1].enabled);
        assert_eq!(imported.warnings.len(), 1);
    }

    #[test]
    fn imports_nested_object_urls_and_skips_requests_without_a_url() {
        let imported = import_json(
            r#"{
              "info": { "name": "blog-user" },
              "item": [
                {
                  "name": "user",
                  "item": [
                    {
                      "name": "login",
                      "request": {
                        "method": "POST",
                        "url": { "raw": "http://127.0.0.1:8088/oauth/login" },
                        "body": { "mode": "raw", "raw": "{\"username\":\"demo\"}" }
                      }
                    },
                    {
                      "name": "missing-url",
                      "request": { "method": "GET" }
                    }
                  ]
                },
                {
                  "name": "Security Oauth2",
                  "item": [
                    {
                      "name": "授权码模式",
                      "request": {
                        "method": "POST",
                        "url": { "raw": "http://localhost:8088/oauth/token" }
                      }
                    },
                    {
                      "name": "授权码模式",
                      "request": {
                        "method": "POST",
                        "url": { "raw": "http://localhost:8088/oauth/token?grant_type=authorization_code" }
                      }
                    }
                  ]
                }
              ]
            }"#,
        )
        .unwrap();

        assert_eq!(imported.name, "blog-user");
        assert_eq!(imported.requests.len(), 3);
        assert_eq!(imported.requests[0].folder, vec!["user"]);
        assert_eq!(
            imported.requests[0].spec.url,
            "http://127.0.0.1:8088/oauth/login"
        );
        assert!(matches!(
            imported.requests[0].spec.body,
            Body::Json { ref content } if content == "{\"username\":\"demo\"}"
        ));
        assert_eq!(
            imported
                .requests
                .iter()
                .filter(|request| request.name == "授权码模式")
                .count(),
            2
        );
        assert!(imported
            .warnings
            .iter()
            .any(|warning| warning.contains("missing-url")));
    }

    #[test]
    fn imports_apifox_base_url_and_object_url_parameters() {
        let imported = import_json(
            r#"{
              "info": { "name": "Apifox" },
              "item": [{
                "name": "fixer",
                "request": {
                  "method": "GET",
                  "url": {
                    "raw": "{{baseURL}}/api/latest",
                    "query": [
                      {"key": "access_key", "value": "secret", "description": "API key"},
                      {"key": "base", "value": "USD", "disabled": true}
                    ],
                    "variable": {"key": "version", "value": "v1", "description": "API version"}
                  }
                }
              }]
            }"#,
        )
        .unwrap();

        let spec = &imported.requests[0].spec;
        assert_eq!(spec.url, "/api/latest");
        assert_eq!(spec.params.len(), 2);
        assert_eq!(spec.params[0].key, "access_key");
        assert_eq!(spec.params[0].value, "secret");
        assert_eq!(spec.params[0].description.as_deref(), Some("API key"));
        assert!(!spec.params[1].enabled);
        assert_eq!(spec.path_params.len(), 1);
        assert_eq!(spec.path_params[0].key, "version");
        assert_eq!(spec.path_params[0].value, "v1");
        assert_eq!(
            spec.path_params[0].description.as_deref(),
            Some("API version")
        );
    }
}
