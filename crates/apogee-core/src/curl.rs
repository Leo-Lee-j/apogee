use crate::error::{AppError, ErrorKind, Hint};
use crate::model::{KeyValue, ValueKind};
use crate::request::{AuthConfig, Body, RequestSettings, RequestSpec};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use curl_parser::ParsedRequest;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurlImportResult {
    pub spec: RequestSpec,
    pub warnings: Vec<String>,
}

pub fn import(command: &str) -> Result<CurlImportResult, AppError> {
    let source = command.trim();
    if source.is_empty() {
        return Err(AppError::new(ErrorKind::Parse, "请粘贴一条 cURL 命令")
            .hint(Hint::new("命令应以 curl 开头")));
    }
    let parsed = std::panic::catch_unwind(|| ParsedRequest::from_str(source))
        .map_err(|_| {
            AppError::new(ErrorKind::Parse, "无法解析 cURL 命令")
                .detail("解析器拒绝了不完整或冲突的参数。")
                .hint(Hint::new("检查 Header 是否包含冒号，以及引号是否闭合"))
        })?
        .map_err(|error| {
            AppError::new(ErrorKind::Parse, "无法解析 cURL 命令")
                .detail(error.to_string())
                .hint(Hint::new("检查引号、换行续写和选项拼写"))
        })?;

    let parsed_url = Url::parse(&parsed.url.to_string()).map_err(|error| {
        AppError::new(ErrorKind::Parse, "cURL 中的 URL 无效").detail(error.to_string())
    })?;
    let params = parsed_url
        .query_pairs()
        .map(|(key, value)| row(key.into_owned(), value.into_owned()))
        .collect();
    let mut base_url = parsed_url;
    base_url.set_query(None);

    let mut auth = AuthConfig::None;
    let mut headers = Vec::new();
    let mut content_type = String::new();
    for (name, value) in &parsed.headers {
        let key = name.as_str().to_string();
        let value = value
            .to_str()
            .map_err(|error| {
                AppError::new(ErrorKind::Parse, format!("Header {key} 不是有效文本"))
                    .detail(error.to_string())
            })?
            .to_string();
        if key.eq_ignore_ascii_case("authorization") {
            if let Some(token) = value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
            {
                auth = AuthConfig::Bearer {
                    token: token.to_string(),
                };
                continue;
            }
            if let Some(encoded) = value.strip_prefix("Basic ") {
                if let Ok(decoded) = STANDARD.decode(encoded) {
                    if let Ok(credentials) = String::from_utf8(decoded) {
                        let (username, password) =
                            credentials.split_once(':').unwrap_or((&credentials, ""));
                        auth = AuthConfig::Basic {
                            username: username.to_string(),
                            password: password.to_string(),
                        };
                        continue;
                    }
                }
            }
        }
        if key.eq_ignore_ascii_case("content-type") {
            content_type = value.clone();
        }
        headers.push(row(key, value));
    }

    let body = if parsed.body.is_empty() {
        Body::None
    } else if content_type
        .to_ascii_lowercase()
        .contains("application/json")
    {
        Body::Json {
            content: parsed.body.last().cloned().unwrap_or_default(),
        }
    } else if content_type
        .to_ascii_lowercase()
        .contains("application/x-www-form-urlencoded")
    {
        let joined = parsed.body.join("&");
        Body::UrlEncoded {
            rows: url::form_urlencoded::parse(joined.as_bytes())
                .map(|(key, value)| row(key.into_owned(), value.into_owned()))
                .collect(),
        }
    } else {
        Body::Text {
            content: parsed.body.join("&"),
            content_type: if content_type.is_empty() {
                "text/plain".into()
            } else {
                content_type
            },
        }
    };

    Ok(CurlImportResult {
        spec: RequestSpec {
            method: parsed.method.as_str().to_string(),
            url: base_url.to_string(),
            params,
            path_params: Vec::new(),
            headers,
            body,
            auth,
            settings: RequestSettings {
                verify_tls: Some(!parsed.insecure),
                ..Default::default()
            },
        },
        warnings: Vec::new(),
    })
}

fn row(key: String, value: String) -> KeyValue {
    KeyValue {
        enabled: true,
        key,
        value,
        description: None,
        kind: ValueKind::Text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_json_query_bearer_and_tls_setting() {
        let result = import(r#"curl -k -X POST 'https://api.example.test/users?q=hello%20world' -H 'Content-Type: application/json' -H 'Authorization: Bearer secret' -d '{"name":"Ada"}'"#).unwrap();
        assert_eq!(result.spec.method, "POST");
        assert_eq!(result.spec.url, "https://api.example.test/users");
        assert_eq!(result.spec.params[0].value, "hello world");
        assert!(matches!(result.spec.auth, AuthConfig::Bearer { ref token } if token == "secret"));
        assert!(matches!(result.spec.body, Body::Json { ref content } if content.contains("Ada")));
        assert_eq!(result.spec.settings.verify_tls, Some(false));
    }

    #[test]
    fn malformed_command_returns_actionable_error() {
        let error = import("curl -H '").unwrap_err();
        assert_eq!(error.kind, ErrorKind::Parse);
        assert!(!error.hints.is_empty());
    }
}
