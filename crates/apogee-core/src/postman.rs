use crate::model::KeyValue;
use crate::request::{
    ApiKeyIn, AuthConfig, Body, JwtAlgorithm, OAuth1SignatureMethod, RequestSpec,
};
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
    let inherited_auth = import_auth(root.get("auth"), &AuthConfig::None, &mut warnings);
    walk_items(
        root.get("item").and_then(|v| v.as_array()),
        &mut Vec::new(),
        &mut requests,
        &mut warnings,
        &inherited_auth,
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
    inherited_auth: &AuthConfig,
) {
    for item in items.into_iter().flatten() {
        if let Some(children) = item.get("item").and_then(|v| v.as_array()) {
            if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                folder.push(name.into());
            }
            let folder_auth = import_auth(item.get("auth"), inherited_auth, warnings);
            walk_items(Some(children), folder, out, warnings, &folder_auth);
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
        let body = import_body(request.get("body"));
        let auth = import_auth(request.get("auth"), inherited_auth, warnings);
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
                url: normalize_apifox_base_url(&if url_value
                    .and_then(|v| v.get("query"))
                    .is_some_and(|v| v.is_array() || v.is_object())
                {
                    // Object query rows are authoritative, including disabled rows. Keeping
                    // raw query as well would duplicate parameters and change OAuth signatures.
                    let (base, fragment) = url
                        .split_once('#')
                        .map_or((url.as_str(), None), |(base, fragment)| {
                            (base, Some(fragment))
                        });
                    let base = base.split_once('?').map_or(base, |(base, _)| base);
                    fragment
                        .map_or_else(|| base.to_string(), |fragment| format!("{base}#{fragment}"))
                } else {
                    url.clone()
                }),
                params,
                path_params,
                headers,
                body,
                auth,
                settings: Default::default(),
            },
        });
    }
}
/// Postman v2.0 uses an object; v2.1 uses an array of { key, value, type }.
fn auth_field(auth: &serde_json::Value, kind: &str, key: &str) -> Option<String> {
    let config = auth.get(kind)?;
    if let Some(array) = config.as_array() {
        return array
            .iter()
            .find(|v| v.get("key").and_then(|v| v.as_str()) == Some(key))
            .map(|v| value_text(v.get("value")));
    }
    config.get(key).map(|v| value_text(Some(v)))
}

fn import_auth(
    value: Option<&serde_json::Value>,
    inherited: &AuthConfig,
    warnings: &mut Vec<String>,
) -> AuthConfig {
    let Some(auth) = value.filter(|v| !v.is_null()) else {
        return inherited.clone();
    };
    match parse_auth(auth) {
        Ok(AuthConfig::Inherit) => inherited.clone(),
        Ok(auth) => auth,
        Err(message) => {
            // Static messages only: do not print credentials or arbitrary import contents.
            warnings.push(message.into());
            AuthConfig::None
        }
    }
}

fn parse_auth(auth: &serde_json::Value) -> Result<AuthConfig, &'static str> {
    let kind = auth
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("inherit");
    let field = |key| auth_field(auth, kind, key).unwrap_or_default();
    let default_field =
        |key, default: &str| auth_field(auth, kind, key).unwrap_or_else(|| default.into());
    Ok(match kind {
        "inherit" => AuthConfig::Inherit,
        "noauth" => AuthConfig::None,
        "basic" => AuthConfig::Basic {
            username: field("username"),
            password: field("password"),
        },
        "bearer" => AuthConfig::Bearer {
            token: field("token"),
        },
        "apikey" => AuthConfig::ApiKey {
            key: field("key"),
            value: field("value"),
            location: match field("in").as_str() {
                "header" | "" => ApiKeyIn::Header,
                "query" => ApiKeyIn::Query,
                "cookie" => ApiKeyIn::Cookie,
                _ => return Err("Postman API Key 位置不受支持，该层已导入为无鉴权，请手动配置"),
            },
        },
        "oauth1" => {
            if default_field("addParamsToHeader", "true") != "true"
                || field("addEmptyParamsToSign") == "true"
            {
                return Err("Postman OAuth 1.0 仅支持 Header 模式且不签名空协议参数，该层已导入为无鉴权，请手动配置");
            }
            let signature_method = match default_field("signatureMethod", "HMAC-SHA1").as_str() {
                "HMAC-SHA1" => OAuth1SignatureMethod::HmacSha1,
                "HMAC-SHA256" => OAuth1SignatureMethod::HmacSha256,
                _ => {
                    return Err(
                        "Postman OAuth 1.0 签名算法不受支持，该层已导入为无鉴权，请手动配置",
                    )
                }
            };
            AuthConfig::OAuth1 {
                consumer_key: field("consumerKey"),
                consumer_secret: field("consumerSecret"),
                token: field("token"),
                token_secret: field("tokenSecret"),
                signature_method,
                realm: field("realm"),
                callback: field("callback"),
                verifier: field("verifier"),
                nonce: field("nonce"),
                timestamp: field("timestamp"),
            }
        }
        "oauth2" => {
            if default_field("addTokenTo", "header") != "header" {
                return Err("Postman OAuth 2.0 仅支持 Header 模式，该层已导入为无鉴权，请手动配置");
            }
            let token_request = if matches!(
                field("grant_type").as_str(),
                "client_credentials"
                    | "refresh_token"
                    | "authorization_code"
                    | "authorization_code_with_pkce"
            ) && !field("accessTokenUrl").is_empty()
                && !field("clientId").is_empty()
            {
                let client_auth = match default_field("client_authentication", "header").as_str() {
                    "header" => crate::oauth2::OAuth2ClientAuth::Basic,
                    "body" => crate::oauth2::OAuth2ClientAuth::Body,
                    _ => return Err(
                        "Postman OAuth 2.0 客户端凭据模式不受支持，该层已导入为无鉴权，请手动配置",
                    ),
                };
                Some(Box::new(crate::oauth2::OAuth2TokenRequest {
                    token_url: field("accessTokenUrl"),
                    client_id: field("clientId"),
                    client_secret: field("clientSecret"),
                    scope: field("scope"),
                    refresh_token: field("refreshToken"),
                    client_auth,
                    grant_type: if matches!(
                        field("grant_type").as_str(),
                        "authorization_code" | "authorization_code_with_pkce"
                    ) {
                        crate::oauth2::OAuth2Flow::AuthorizationCode
                    } else {
                        crate::oauth2::OAuth2Flow::ClientCredentials
                    },
                    authorization_url: field("authUrl"),
                    redirect_uri: field("redirect_uri"),
                }))
            } else {
                None
            };
            if field("accessToken").is_empty() && token_request.is_none() {
                return Err("Postman OAuth 2.0 需要已有 Access Token 或可执行 Token 获取配置，该层已导入为无鉴权，请手动配置");
            }
            AuthConfig::OAuth2 {
                access_token: field("accessToken"),
                header_prefix: default_field("headerPrefix", "Bearer"),
                token_request,
                obtained_at: None,
                expires_at: None,
                auto_refresh: false,
                token_context: None,
            }
        }
        "jwt" => {
            if default_field("addTokenTo", "header") != "header"
                || field("isSecretBase64Encoded") == "true"
            {
                return Err(
                    "Postman JWT 仅支持 Header 模式和 UTF-8 Secret，该层已导入为无鉴权，请手动配置",
                );
            }
            let algorithm = match default_field("algorithm", "HS256").as_str() {
                "HS256" => JwtAlgorithm::HS256,
                "HS384" => JwtAlgorithm::HS384,
                "HS512" => JwtAlgorithm::HS512,
                _ => return Err("Postman JWT 签名算法不受支持，该层已导入为无鉴权，请手动配置"),
            };
            AuthConfig::Jwt {
                secret: field("secret"),
                payload: default_field("payload", "{}"),
                header: default_field("header", "{}"),
                algorithm,
                header_prefix: default_field("headerPrefix", "Bearer"),
            }
        }
        _ => return Err("Postman 鉴权类型不受支持，该层已导入为无鉴权，请手动配置"),
    })
}

fn import_body(value: Option<&serde_json::Value>) -> Body {
    let Some(value) = value else {
        return Body::None;
    };
    match value.get("mode").and_then(|v| v.as_str()) {
        Some("urlencoded") => Body::UrlEncoded {
            rows: rows(value.get("urlencoded")),
        },
        Some("raw") | None => {
            let Some(raw) = value.get("raw").and_then(|v| v.as_str()) else {
                return Body::None;
            };
            match value
                .pointer("/options/raw/language")
                .and_then(|v| v.as_str())
                .unwrap_or("json")
            {
                "json" => Body::Json {
                    content: raw.into(),
                },
                "xml" => Body::Text {
                    content: raw.into(),
                    content_type: "application/xml".into(),
                },
                "html" => Body::Text {
                    content: raw.into(),
                    content_type: "text/html".into(),
                },
                _ => Body::Text {
                    content: raw.into(),
                    content_type: "text/plain".into(),
                },
            }
        }
        _ => Body::None,
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
                value: value_text(v.get("value")),
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
    fn imports_pkce_configuration_without_ever_importing_ephemeral_codes() {
        let auth = parse_auth(&serde_json::json!({"type":"oauth2", "oauth2":[
            {"key":"grant_type","value":"authorization_code_with_pkce"},
            {"key":"accessTokenUrl","value":"https://provider.test/token"},
            {"key":"authUrl","value":"https://provider.test/authorize"},
            {"key":"redirect_uri","value":"http://127.0.0.1:53682/cb"},
            {"key":"clientId","value":"public-client"}, {"key":"code_verifier","value":"private-verifier"}
        ]})).unwrap();
        assert!(
            matches!(&auth, AuthConfig::OAuth2 { token_request: Some(config), .. }
            if config.grant_type == crate::oauth2::OAuth2Flow::AuthorizationCode && config.redirect_uri.ends_with("/cb") && config.client_secret.is_empty())
        );
        let json = serde_json::to_string(&auth).unwrap();
        assert!(!json.contains("private-verifier"));
        let yaml = crate::yaml::to_string(&auth).unwrap();
        let roundtrip: AuthConfig = crate::yaml::from_str(&yaml).unwrap();
        assert_eq!(
            serde_json::to_value(auth).unwrap(),
            serde_json::to_value(roundtrip).unwrap()
        );
    }

    #[test]
    fn imports_oauth2_client_credentials_without_existing_token() {
        let auth=parse_auth(&serde_json::json!({"type":"oauth2","oauth2":[{"key":"grant_type","value":"client_credentials"},
            {"key":"accessTokenUrl","value":"{{tokenUrl}}"},{"key":"clientId","value":"{{clientId}}"},
            {"key":"clientSecret","value":"{{clientSecret}}"},{"key":"client_authentication","value":"body"}]})).unwrap();
        assert!(
            matches!(auth,AuthConfig::OAuth2 {access_token,token_request:Some(config),..}
            if access_token.is_empty() && config.client_secret=="{{clientSecret}}" && config.client_auth==crate::oauth2::OAuth2ClientAuth::Body)
        );
    }

    #[test]
    fn imported_query_rows_are_sent_once_and_disabled_raw_query_is_not_signed() {
        let imported=import_json(r#"{"item":[{"request":{"url":{"raw":"https://example.test/?x=1&x=2&disabled=secret#part",
            "query":[{"key":"x","value":"1"},{"key":"x","value":"2"},{"key":"disabled","value":"secret","disabled":true}]}}}]}"#).unwrap();
        let spec = &imported.requests[0].spec;
        assert_eq!(
            crate::http::build_url(spec).unwrap(),
            "https://example.test/?x=1&x=2#part"
        );
    }

    #[test]
    fn auth_imports_v20_v21_and_flattens_container_inheritance() {
        let imported = import_json(&serde_json::json!({"info":{"name":"Auth"},
            "auth":{"type":"bearer","bearer":{"token":"collection-token"}},
            "item":[{"name":"collection inherit","request":{"url":"https://example.test/"}},
                {"name":"folder","auth":{"type":"oauth1","oauth1":[{"key":"consumerKey","value":"{{key}}"},
                    {"key":"consumerSecret","value":"secret"},{"key":"signatureMethod","value":"HMAC-SHA256"}]},
                "item":[{"name":"inherit","request":{"url":"https://example.test/","body":{"mode":"urlencoded","urlencoded":[{"key":"a","value":"b"}]}}},
                    {"name":"none","request":{"url":"https://example.test/","auth":{"type":"noauth"}}},
                    {"name":"own","request":{"url":"https://example.test/","auth":{"type":"basic","basic":{"username":"u","password":"p"}}}}]}]}).to_string()).unwrap();
        assert!(imported.warnings.is_empty());
        assert!(
            matches!(&imported.requests[0].spec.auth,AuthConfig::Bearer {token} if token=="collection-token")
        );
        assert!(
            matches!(&imported.requests[1].spec.auth,AuthConfig::OAuth1 {consumer_key,signature_method,..} if consumer_key=="{{key}}" && *signature_method==OAuth1SignatureMethod::HmacSha256)
        );
        assert!(matches!(&imported.requests[1].spec.body,Body::UrlEncoded {rows} if rows.len()==1));
        assert!(matches!(&imported.requests[2].spec.auth, AuthConfig::None));
        assert!(
            matches!(&imported.requests[3].spec.auth,AuthConfig::Basic {password,..} if password=="p")
        );
    }
    #[test]
    fn oauth2_jwt_and_api_key_import_and_unsupported_modes_warn_without_secrets() {
        for auth in [
            serde_json::json!({"type":"oauth2","oauth2":{"accessToken":"secret","headerPrefix":"Token"}}),
            serde_json::json!({"type":"jwt","jwt":{"secret":"secret","payload":"{}","algorithm":"HS384"}}),
            serde_json::json!({"type":"apikey","apikey":{"key":"x","value":"secret","in":"cookie"}}),
        ] {
            assert!(!matches!(
                parse_auth(&auth).unwrap(),
                AuthConfig::None | AuthConfig::Inherit
            ));
        }
        for auth in [
            serde_json::json!({"type":"oauth1","oauth1":{"consumerSecret":"private-secret","signatureMethod":"RSA-SHA1"}}),
            serde_json::json!({"type":"oauth2","oauth2":{"accessToken":"private-secret","addTokenTo":"queryParams"}}),
            serde_json::json!({"type":"jwt","jwt":{"secret":"private-secret","algorithm":"none"}}),
            serde_json::json!({"type":"ntlm","ntlm":{"password":"private-secret"}}),
        ] {
            let mut warnings = Vec::new();
            assert!(matches!(
                import_auth(Some(&auth), &AuthConfig::None, &mut warnings),
                AuthConfig::None
            ));
            assert_eq!(warnings.len(), 1);
            assert!(!warnings[0].contains("private-secret"));
        }
    }

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
