//! 请求变量与环境解析。
//!
//! 环境值只在发送前解析，不写回 `RequestSpec`。这样请求 YAML 仍保存
//! `{{variable}}` 模板，切换环境不会污染请求草稿，也不会把某个环境的
//! 实际值意外提交到 Git。

use crate::error::{AppError, ErrorKind, Hint};
use crate::model::KeyValue;
use crate::request::{AuthConfig, Body, RequestSpec};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
};
use uuid::Uuid;

/// 一个工作区环境。值保存在工作区的 environments/<id>.yaml 中。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub variables: Vec<EnvironmentVariable>,
}

/// 环境变量行。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentVariable {
    #[serde(default = "new_variable_id")]
    pub id: Uuid,
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_value: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_secret: bool,
}

fn new_variable_id() -> Uuid {
    Uuid::new_v4()
}

fn is_false(value: &bool) -> bool {
    !value
}

fn default_true() -> bool {
    true
}

const MAX_RESOLUTION_DEPTH: usize = 16;
const MAX_SCOPE_VARIABLES: usize = 256;
const MAX_SCOPE_BYTES: usize = 65_536;
const MAX_VARIABLE_BYTES: usize = 1_048_576;
const MAX_EXPANSION_BYTES: usize = 8_388_608;
const MAX_RESOLUTIONS: usize = 10_000;

fn check_scope_limits(count: usize, size: usize) -> Result<(), AppError> {
    if count > MAX_SCOPE_VARIABLES || size > MAX_SCOPE_BYTES {
        return Err(AppError::new(
            ErrorKind::Var,
            "变量作用域超过 256 条或 65536 字节限制",
        ));
    }
    Ok(())
}

fn check_variable_scope(variables: &[EnvironmentVariable]) -> Result<(), AppError> {
    check_scope_limits(
        variables.len(),
        variables.iter().fold(0usize, |size, item| {
            size.saturating_add(item.key.len())
                .saturating_add(item.value.len())
        }),
    )
}

/// 用一个环境解析请求模板中的全部 `{{name}}` 占位符。
///
/// 解析发生在 HTTP 请求建造之前，因此 URL、query、header、body 和鉴权看到的
/// 是同一份环境快照。未定义、循环引用和超过深度的变量都返回 `ErrorKind::Var`，
/// 不会把原始占位符静默发送到网络。
pub fn resolve_request(
    spec: &RequestSpec,
    environment: Option<&Environment>,
) -> Result<RequestSpec, AppError> {
    Ok(resolve_request_with_metadata(spec, environment)?.spec)
}

/// Resolves the request and retains secret-variable provenance for public request snapshots.
pub fn resolve_request_with_metadata(
    spec: &RequestSpec,
    environment: Option<&Environment>,
) -> Result<ResolvedRequest, AppError> {
    resolve_request_with_scopes(
        spec,
        environment,
        &[],
        &[],
        &[],
        &std::collections::BTreeMap::new(),
    )
}

/// Resolve with layered workspace and per-send variable scopes. Nearest scopes override
/// outer values while duplicates inside one scope are rejected.
pub fn resolve_request_with_scopes(
    spec: &RequestSpec,
    environment: Option<&Environment>,
    global: &[EnvironmentVariable],
    collection: &[EnvironmentVariable],
    folder: &[EnvironmentVariable],
    runtime: &std::collections::BTreeMap<String, String>,
) -> Result<ResolvedRequest, AppError> {
    let mut resolver = Resolver::from_environment(None)?;
    resolver.add_scope(global, "全局")?;
    resolver.add_scope(collection, "集合")?;
    resolver.add_scope(folder, "文件夹")?;
    resolver.add_environment(environment)?;
    resolver.add_runtime(runtime)?;
    let mut resolved = spec.clone();

    // Legacy {{name}} remains Path syntax. Explicit {{env:name}} resolves URL variables
    // without changing existing request files or allowing Path substitution in the authority.
    resolved.url = resolver.resolve_url(&spec.url)?;
    if let Some(environment) = environment {
        resolved.url = with_environment_host(&resolved.url, &environment.host);
    }
    resolved.params = resolver.resolve_rows(&spec.params)?;
    resolved.path_params = resolver.resolve_rows(&spec.path_params)?;
    resolved.headers = resolver.resolve_rows(&spec.headers)?;
    resolved.body = resolver.resolve_body(&spec.body)?;
    resolved.auth = resolver.resolve_auth(&spec.auth)?;

    Ok(ResolvedRequest {
        spec: resolved,
        sensitive_values: resolver.sensitive_values.into_inner(),
    })
}

#[derive(Debug, Clone)]
pub struct ResolvedRequest {
    pub spec: RequestSpec,
    pub sensitive_values: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VariableScopes {
    pub global: Vec<EnvironmentVariable>,
    pub collection: Vec<EnvironmentVariable>,
    pub folder: Vec<EnvironmentVariable>,
    pub runtime: std::collections::BTreeMap<String, String>,
}

/// Borrowed variable snapshot for one execution; never serialized or logged.
pub struct VariableContext<'a> {
    pub environment: Option<&'a Environment>,
    pub scopes: &'a VariableScopes,
}

/// Resolve a bounded group of fields with one expansion budget and one scope snapshot.
pub fn resolve_texts_with_scopes(
    texts: &[&str],
    context: VariableContext<'_>,
) -> Result<Vec<String>, AppError> {
    let mut resolver = Resolver::from_environment(None)?;
    resolver.add_scope(&context.scopes.global, "全局")?;
    resolver.add_scope(&context.scopes.collection, "集合")?;
    resolver.add_scope(&context.scopes.folder, "文件夹")?;
    resolver.add_environment(context.environment)?;
    resolver.add_runtime(&context.scopes.runtime)?;
    texts
        .iter()
        .map(|text| resolver.resolve_text(text))
        .collect()
}

/// 对单段文本做变量解析，供未来的预览命令复用。
pub fn resolve_text(text: &str, environment: Option<&Environment>) -> Result<String, AppError> {
    Resolver::from_environment(environment)?.resolve_text(text)
}

struct VariableValue {
    value: String,
    secret: bool,
}

struct Resolver {
    values: HashMap<String, VariableValue>,
    sensitive_values: RefCell<Vec<String>>,
    resolutions: Cell<usize>,
    expansion_bytes: Cell<usize>,
}

impl Resolver {
    fn add_scope(
        &mut self,
        variables: &[EnvironmentVariable],
        scope: &str,
    ) -> Result<(), AppError> {
        check_variable_scope(variables)?;
        let mut names = std::collections::HashSet::new();
        for variable in variables.iter().filter(|item| item.enabled) {
            let key = variable.key.trim();
            if key.is_empty() {
                continue;
            }
            if key.eq_ignore_ascii_case("host") {
                return Err(AppError::new(ErrorKind::Var, "变量名 host 为保留名称")
                    .detail(format!("{scope}作用域不能声明 host。"))
                    .hint(Hint::new("请使用环境 Host 字段")));
            }
            if !names.insert(key.to_string()) {
                return Err(AppError::new(
                    ErrorKind::Var,
                    format!("{scope}变量名重复：{key}"),
                ));
            }
            self.values.insert(
                key.to_string(),
                VariableValue {
                    value: variable.value.clone(),
                    secret: variable.secret,
                },
            );
        }
        Ok(())
    }

    fn add_runtime(
        &mut self,
        variables: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), AppError> {
        check_scope_limits(
            variables.len(),
            variables.iter().fold(0usize, |size, (key, value)| {
                size.saturating_add(key.len()).saturating_add(value.len())
            }),
        )?;
        let mut names = std::collections::HashSet::new();
        for (key, value) in variables {
            let key = key.trim();
            if key.is_empty() {
                continue;
            }
            if key.eq_ignore_ascii_case("host") {
                return Err(AppError::new(ErrorKind::Var, "变量名 host 为保留名称")
                    .detail("运行时作用域不能声明 host。")
                    .hint(Hint::new("请使用环境 Host 字段")));
            }
            if !names.insert(key.to_string()) {
                return Err(AppError::new(
                    ErrorKind::Var,
                    format!("运行时变量名重复：{key}"),
                ));
            }
            self.values.insert(
                key.to_string(),
                VariableValue {
                    value: value.clone(),
                    secret: false,
                },
            );
        }
        Ok(())
    }

    fn resolve_url(&self, url: &str) -> Result<String, AppError> {
        let mut result = String::new();
        let mut remaining = url;
        while let Some(start) = remaining.find("{{env:") {
            result.push_str(&remaining[..start]);
            let name_start = start + 6;
            let end = remaining[name_start..].find("}}").ok_or_else(|| {
                AppError::new(ErrorKind::Var, "URL 环境变量没有闭合")
                    .hint(Hint::new("使用 {{env:变量名}} 格式"))
            })?;
            let name = remaining[name_start..name_start + end].trim();
            result.push_str(&self.resolve_variable(name, &mut Vec::new(), 0)?);
            remaining = &remaining[name_start + end + 2..];
        }
        result.push_str(remaining);
        Ok(result)
    }

    fn from_environment(environment: Option<&Environment>) -> Result<Self, AppError> {
        let mut resolver = Self {
            values: HashMap::new(),
            sensitive_values: RefCell::new(Vec::new()),
            resolutions: Cell::new(0),
            expansion_bytes: Cell::new(0),
        };
        resolver.add_environment(environment)?;
        Ok(resolver)
    }

    fn add_environment(&mut self, environment: Option<&Environment>) -> Result<(), AppError> {
        if let Some(environment) = environment {
            check_variable_scope(&environment.variables)?;
            if environment
                .variables
                .iter()
                .any(|item| item.enabled && item.key.trim().eq_ignore_ascii_case("host"))
            {
                return Err(
                    AppError::new(ErrorKind::Var, "变量名 host 为保留名称").hint(Hint::new(
                        "请重命名普通变量；host 由当前环境的 Host 字段提供",
                    )),
                );
            }
            let mut names = std::collections::HashSet::new();
            for variable in environment.variables.iter().filter(|item| item.enabled) {
                let key = variable.key.trim();
                if key.is_empty() {
                    continue;
                }
                if !names.insert(key.to_string()) {
                    return Err(
                        AppError::new(ErrorKind::Var, format!("环境变量名重复：{key}"))
                            .detail(format!("环境「{}」中有多个同名变量。", environment.name))
                            .hint(Hint::new("删除重复变量，或为变量改名后再发送")),
                    );
                }
                self.values.insert(
                    key.to_string(),
                    VariableValue {
                        value: variable.value.clone(),
                        secret: variable.secret,
                    },
                );
            }
            let host = environment.host.trim().trim_end_matches('/');
            if !host.is_empty() {
                self.values.insert(
                    "host".to_string(),
                    VariableValue {
                        value: host.to_string(),
                        secret: false,
                    },
                );
            }
        }
        Ok(())
    }

    fn resolve_text(&self, text: &str) -> Result<String, AppError> {
        let mut stack = Vec::new();
        self.resolve_text_inner(text, &mut stack, 0)
    }

    fn resolve_text_inner(
        &self,
        text: &str,
        stack: &mut Vec<String>,
        depth: usize,
    ) -> Result<String, AppError> {
        let mut output = String::with_capacity(text.len());
        let mut cursor = 0;

        while let Some(relative_start) = text[cursor..].find("{{") {
            let start = cursor + relative_start;
            output.push_str(&text[cursor..start]);

            let value_start = start + 2;
            let relative_end = text[value_start..].find("}}").ok_or_else(|| {
                AppError::new(ErrorKind::Var, "变量占位符没有闭合")
                    .detail(format!(
                        "从第 {} 个字符开始的 `{{{{` 没有对应的 `}}}}`。",
                        start + 1
                    ))
                    .hint(Hint::new("检查变量语法，格式应为 {{variable}}"))
            })?;
            let end = value_start + relative_end;
            let name = text[value_start..end].trim();
            if name.is_empty() {
                return Err(AppError::new(ErrorKind::Var, "变量名不能为空")
                    .hint(Hint::new("在 {{ 和 }} 之间填写变量名")));
            }

            output.push_str(&self.resolve_variable(name, stack, depth)?);
            cursor = end + 2;
        }

        output.push_str(&text[cursor..]);
        Ok(output)
    }

    fn resolve_variable(
        &self,
        name: &str,
        stack: &mut Vec<String>,
        depth: usize,
    ) -> Result<String, AppError> {
        let resolutions = self.resolutions.get().saturating_add(1);
        if resolutions > MAX_RESOLUTIONS {
            return Err(AppError::new(ErrorKind::Var, "变量展开次数超过限制"));
        }
        self.resolutions.set(resolutions);
        if let Some(start) = stack.iter().position(|item| item == name) {
            let mut chain = stack[start..].to_vec();
            chain.push(name.to_string());
            return Err(AppError::new(
                ErrorKind::Var,
                format!("变量循环引用：{}", chain.join(" → ")),
            )
            .hint(Hint::new("检查环境变量之间的引用关系，移除循环引用")));
        }
        if depth >= MAX_RESOLUTION_DEPTH {
            return Err(AppError::new(
                ErrorKind::Var,
                format!("变量解析嵌套超过 {MAX_RESOLUTION_DEPTH} 层"),
            )
            .hint(Hint::new("减少环境变量之间的层层引用")));
        }

        let variable = self.values.get(name).ok_or_else(|| {
            AppError::new(ErrorKind::Var, format!("变量未定义：{name}"))
                .detail("请求中引用了当前环境不存在或已禁用的变量。")
                .hint(Hint::new("选择包含该变量的环境，或在环境变量面板中补充值"))
        })?;

        let raw = variable.value.clone();
        let secret = variable.secret;
        if secret && raw.is_empty() {
            return Err(
                AppError::new(ErrorKind::Var, format!("密钥变量尚未设置：{name}"))
                    .hint(Hint::new("请重新设置该密钥变量后再发送")),
            );
        }
        stack.push(name.to_string());
        let result = self.resolve_text_inner(&raw, stack, depth + 1);
        stack.pop();
        let result = result?;
        let expansion_bytes = self.expansion_bytes.get().saturating_add(result.len());
        if result.len() > MAX_VARIABLE_BYTES || expansion_bytes > MAX_EXPANSION_BYTES {
            return Err(AppError::new(ErrorKind::Var, "变量展开大小超过限制")
                .hint(Hint::new("减少变量的重复引用或将大段内容直接放入请求体")));
        }
        self.expansion_bytes.set(expansion_bytes);
        if secret {
            let mut sensitive_values = self.sensitive_values.borrow_mut();
            if !result.is_empty() && !sensitive_values.contains(&result) {
                sensitive_values.push(result.clone());
            }
        }
        Ok(result)
    }

    fn resolve_rows(&self, rows: &[KeyValue]) -> Result<Vec<KeyValue>, AppError> {
        rows.iter()
            .map(|row| {
                // 禁用行不会进入请求，也不应因为其中残留的模板阻止发送。
                if !row.enabled {
                    return Ok(row.clone());
                }
                let mut resolved = row.clone();
                resolved.key = self.resolve_text(&row.key)?;
                resolved.value = self.resolve_text(&row.value)?;
                if resolved.kind == crate::model::ValueKind::Text
                    && self.sensitive_values.borrow().iter().any(|value| {
                        !value.is_empty()
                            && (resolved.value.contains(value) || resolved.key.contains(value))
                    })
                {
                    resolved.kind = crate::model::ValueKind::Secret;
                }
                Ok(resolved)
            })
            .collect()
    }

    fn resolve_body(&self, body: &Body) -> Result<Body, AppError> {
        Ok(match body {
            Body::None => Body::None,
            Body::Json { content } => Body::Json {
                content: self.resolve_text(content)?,
            },
            Body::Text {
                content,
                content_type,
            } => Body::Text {
                content: self.resolve_text(content)?,
                content_type: self.resolve_text(content_type)?,
            },
            Body::FormData { rows } => Body::FormData {
                rows: self.resolve_rows(rows)?,
            },
            Body::UrlEncoded { rows } => Body::UrlEncoded {
                rows: self.resolve_rows(rows)?,
            },
            Body::Binary { path } => Body::Binary {
                path: std::path::PathBuf::from(self.resolve_text(&path.to_string_lossy())?),
            },
            Body::GraphQl {
                query,
                variables,
                operation_name,
            } => Body::GraphQl {
                query: self.resolve_text(query)?,
                variables: self.resolve_text(variables)?,
                operation_name: operation_name
                    .as_deref()
                    .map(|value| self.resolve_text(value))
                    .transpose()?,
            },
        })
    }

    fn resolve_auth(&self, auth: &AuthConfig) -> Result<AuthConfig, AppError> {
        Ok(match auth {
            AuthConfig::Inherit => AuthConfig::Inherit,
            AuthConfig::None => AuthConfig::None,
            AuthConfig::Basic { username, password } => AuthConfig::Basic {
                username: self.resolve_text(username)?,
                password: self.resolve_text(password)?,
            },
            AuthConfig::Bearer { token } => AuthConfig::Bearer {
                token: self.resolve_text(token)?,
            },
            AuthConfig::OAuth1 {
                consumer_key,
                consumer_secret,
                token,
                token_secret,
                signature_method,
                realm,
                callback,
                verifier,
                nonce,
                timestamp,
            } => AuthConfig::OAuth1 {
                consumer_key: self.resolve_text(consumer_key)?,
                consumer_secret: self.resolve_text(consumer_secret)?,
                token: self.resolve_text(token)?,
                token_secret: self.resolve_text(token_secret)?,
                signature_method: *signature_method,
                realm: self.resolve_text(realm)?,
                callback: self.resolve_text(callback)?,
                verifier: self.resolve_text(verifier)?,
                nonce: self.resolve_text(nonce)?,
                timestamp: self.resolve_text(timestamp)?,
            },
            AuthConfig::OAuth2 {
                access_token,
                header_prefix,
                token_request,
                obtained_at,
                expires_at,
                auto_refresh,
                token_context,
            } => AuthConfig::OAuth2 {
                access_token: self.resolve_text(access_token)?,
                header_prefix: self.resolve_text(header_prefix)?,
                token_request: token_request.clone(),
                obtained_at: *obtained_at,
                expires_at: *expires_at,
                auto_refresh: *auto_refresh,
                token_context: token_context.clone(),
            },
            AuthConfig::Jwt {
                secret,
                payload,
                header,
                algorithm,
                header_prefix,
            } => AuthConfig::Jwt {
                secret: self.resolve_text(secret)?,
                payload: self.resolve_text(payload)?,
                header: self.resolve_text(header)?,
                algorithm: *algorithm,
                header_prefix: self.resolve_text(header_prefix)?,
            },
            AuthConfig::ApiKey {
                key,
                value,
                location,
            } => AuthConfig::ApiKey {
                key: self.resolve_text(key)?,
                value: self.resolve_text(value)?,
                location: *location,
            },
        })
    }
}

fn with_environment_host(url: &str, host: &str) -> String {
    let url = url.trim();
    let host = host.trim().trim_end_matches('/');
    if url.is_empty() || host.is_empty() || url::Url::parse(url).is_ok() {
        return url.to_string();
    }
    if url.starts_with('/') {
        format!("{host}{url}")
    } else {
        format!("{host}/{url}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::RequestSpec;

    fn environment(variables: &[(&str, &str)]) -> Environment {
        Environment {
            id: Uuid::new_v4(),
            name: "测试环境".into(),
            host: String::new(),
            variables: variables
                .iter()
                .map(|(key, value)| EnvironmentVariable {
                    id: Uuid::new_v4(),
                    key: (*key).into(),
                    value: (*value).into(),
                    enabled: true,
                    secret: false,
                    has_value: false,
                    clear_secret: false,
                })
                .collect(),
        }
    }

    #[test]
    fn resolves_all_new_auth_fields_without_changing_algorithms() {
        let env = environment(&[("v", "resolved")]);
        let resolver = Resolver::from_environment(Some(&env)).unwrap();
        let auth = serde_json::json!({"type":"oauth1","consumerKey":"{{v}}","consumerSecret":"{{v}}",
            "token":"{{v}}","tokenSecret":"{{v}}","realm":"{{v}}","callback":"{{v}}","verifier":"{{v}}",
            "nonce":"{{v}}","timestamp":"{{v}}","signatureMethod":"HMAC-SHA256"});
        let resolved = resolver
            .resolve_auth(&serde_json::from_value(auth).unwrap())
            .unwrap();
        let resolved = serde_json::to_value(resolved).unwrap();
        for key in [
            "consumerKey",
            "consumerSecret",
            "token",
            "tokenSecret",
            "realm",
            "callback",
            "verifier",
            "nonce",
            "timestamp",
        ] {
            assert_eq!(resolved[key], "resolved");
        }
        assert_eq!(resolved["signatureMethod"], "HMAC-SHA256");
        for auth in [
            serde_json::json!({"type":"oauth2","accessToken":"{{v}}","headerPrefix":"{{v}}"}),
            serde_json::json!({"type":"jwt","secret":"{{v}}","payload":"{{v}}","header":"{{v}}","headerPrefix":"{{v}}","algorithm":"HS512"}),
        ] {
            let resolved = serde_json::to_value(
                resolver
                    .resolve_auth(&serde_json::from_value(auth).unwrap())
                    .unwrap(),
            )
            .unwrap();
            for (key, value) in resolved.as_object().unwrap() {
                if key != "type" && key != "algorithm" {
                    assert_eq!(value, "resolved");
                }
            }
        }
    }

    #[test]
    fn resolves_nested_variables() {
        let mut env = environment(&[("base", "https://{{host}}/v1")]);
        env.host = "api.example.test".into();
        assert_eq!(
            resolve_text("{{base}}/users", Some(&env)).unwrap(),
            "https://api.example.test/v1/users"
        );
    }

    #[test]
    fn runtime_scope_rejects_excessive_count_and_bytes() {
        let mut runtime = std::collections::BTreeMap::new();
        for i in 0..257 {
            runtime.insert(format!("v{i}"), String::new());
        }
        let mut resolver = Resolver::from_environment(None).unwrap();
        assert!(resolver.add_runtime(&runtime).is_err());
        runtime.clear();
        runtime.insert("large".into(), "x".repeat(MAX_SCOPE_BYTES));
        assert!(resolver.add_runtime(&runtime).is_err());
    }

    #[test]
    fn secret_file_path_preserves_multipart_file_semantics() {
        let mut env = environment(&[("file", "synthetic-upload.txt")]);
        env.variables[0].secret = true;
        let resolver = Resolver::from_environment(Some(&env)).unwrap();
        let body = resolver
            .resolve_body(&Body::FormData {
                rows: vec![KeyValue {
                    key: "upload".into(),
                    value: "{{file}}".into(),
                    enabled: true,
                    description: None,
                    kind: crate::model::ValueKind::File,
                }],
            })
            .unwrap();
        let Body::FormData { rows } = body else {
            panic!("multipart body changed type")
        };
        assert_eq!(rows[0].kind, crate::model::ValueKind::File);
        assert_eq!(rows[0].value, "synthetic-upload.txt");
    }

    #[test]
    fn recursive_expansion_is_bounded_for_large_and_empty_values() {
        let mut large = environment(&[("leaf", "")]);
        large.variables[0].value = "x".repeat(4096);
        let mut first = large.variables[0].clone();
        first.key = "first".into();
        first.value = "{{leaf}}".repeat(32);
        let mut second = first.clone();
        second.key = "second".into();
        second.value = "{{first}}".repeat(32);
        large.variables.extend([first, second]);
        assert!(resolve_text("{{second}}", Some(&large))
            .unwrap_err()
            .message
            .contains("大小超过限制"));

        let mut empty = environment(&[("leaf", "")]);
        for i in 0..7 {
            let mut variable = empty.variables[0].clone();
            variable.key = format!("v{i}");
            let prior = if i == 0 {
                "leaf".into()
            } else {
                format!("v{}", i - 1)
            };
            variable.value = format!("{{{{{prior}}}}}").repeat(16);
            empty.variables.push(variable);
        }
        assert!(resolve_text("{{v6}}", Some(&empty))
            .unwrap_err()
            .message
            .contains("次数超过限制"));
    }

    #[test]
    fn unresolved_enabled_secret_fails_but_disabled_secret_does_not_block() {
        let mut env = environment(&[("token", "unused")]);
        env.variables[0].secret = true;
        env.variables[0].value.clear();
        let spec = RequestSpec {
            method: "GET".into(),
            url: "https://example.test/{{env:token}}".into(),
            params: vec![],
            path_params: vec![],
            headers: vec![],
            body: Body::None,
            auth: AuthConfig::None,
            settings: Default::default(),
        };
        assert!(resolve_request(&spec, Some(&env)).is_err());
        env.variables[0].enabled = false;
        assert!(resolve_request(&spec, Some(&env)).is_err()); // the referenced variable remains undefined
        let unused = RequestSpec {
            url: "https://example.test/".into(),
            ..spec
        };
        assert!(resolve_request(&unused, Some(&env)).is_ok());
    }

    #[test]
    fn nearest_scope_overrides_and_secrets_keep_provenance() {
        let mut environment = environment(&[("x", "environment"), ("token", "synthetic-secret")]);
        environment.variables[1].secret = true;
        let variable = |key: &str, value: &str| EnvironmentVariable {
            id: Uuid::new_v4(),
            key: key.into(),
            value: value.into(),
            enabled: true,
            secret: false,
            has_value: false,
            clear_secret: false,
        };
        let mut spec = RequestSpec {
            method: "GET".into(),
            url: "https://example.test/{{env:token}}".into(),
            params: Vec::new(),
            path_params: Vec::new(),
            headers: Vec::new(),
            body: Body::None,
            auth: AuthConfig::None,
            settings: Default::default(),
        };
        spec.headers.push(KeyValue {
            key: "X-Custom".into(),
            value: "prefix-{{token}}".into(),
            enabled: true,
            description: None,
            kind: crate::model::ValueKind::Text,
        });
        let mut runtime = std::collections::BTreeMap::new();
        runtime.insert("x".into(), "runtime".into());
        let resolved = resolve_request_with_scopes(
            &spec,
            Some(&environment),
            &[variable("x", "global")],
            &[variable("x", "collection")],
            &[variable("x", "folder")],
            &runtime,
        )
        .unwrap();
        assert_eq!(
            resolve_text_with_scopes(
                "{{x}}",
                &[variable("x", "global")],
                &[variable("x", "collection")],
                &[variable("x", "folder")],
                Some(&environment),
                &runtime
            )
            .unwrap(),
            "runtime"
        );
        assert!(resolved
            .sensitive_values
            .contains(&"synthetic-secret".to_string()));
        assert_eq!(
            resolved.spec.headers[0].kind,
            crate::model::ValueKind::Secret
        );
    }

    fn resolve_text_with_scopes(
        text: &str,
        global: &[EnvironmentVariable],
        collection: &[EnvironmentVariable],
        folder: &[EnvironmentVariable],
        environment: Option<&Environment>,
        runtime: &std::collections::BTreeMap<String, String>,
    ) -> Result<String, AppError> {
        let spec = RequestSpec {
            method: "GET".into(),
            url: "https://example.test/".into(),
            params: Vec::new(),
            path_params: Vec::new(),
            headers: Vec::new(),
            body: Body::Text {
                content: text.into(),
                content_type: "text/plain".into(),
            },
            auth: AuthConfig::None,
            settings: Default::default(),
        };
        let resolved =
            resolve_request_with_scopes(&spec, environment, global, collection, folder, runtime)?;
        match resolved.spec.body {
            Body::Text { content, .. } => Ok(content),
            _ => unreachable!(),
        }
    }

    #[test]
    fn rejects_host_name_in_every_persisted_scope() {
        let reserved = EnvironmentVariable {
            id: Uuid::new_v4(),
            key: "Host".into(),
            value: "https://other.test".into(),
            enabled: true,
            secret: false,
            has_value: false,
            clear_secret: false,
        };
        let spec = RequestSpec {
            method: "GET".into(),
            url: "/".into(),
            params: Vec::new(),
            path_params: Vec::new(),
            headers: Vec::new(),
            body: Body::None,
            auth: AuthConfig::None,
            settings: Default::default(),
        };
        assert!(resolve_request_with_scopes(
            &spec,
            None,
            &[reserved],
            &[],
            &[],
            &Default::default()
        )
        .is_err());

        let mut runtime = std::collections::BTreeMap::new();
        runtime.insert("HOST".into(), "https://runtime.test".into());
        assert!(resolve_request_with_scopes(&spec, None, &[], &[], &[], &runtime).is_err());
    }

    #[test]
    fn disabled_inner_scope_does_not_shadow_outer_value() {
        let mut disabled = EnvironmentVariable {
            id: Uuid::new_v4(),
            key: "x".into(),
            value: "inner".into(),
            enabled: false,
            secret: false,
            has_value: false,
            clear_secret: false,
        };
        let parent = EnvironmentVariable {
            id: Uuid::new_v4(),
            key: "x".into(),
            value: "parent".into(),
            enabled: true,
            secret: false,
            has_value: false,
            clear_secret: false,
        };
        assert_eq!(
            resolve_text_with_scopes(
                "{{x}}",
                &[parent],
                &[],
                &[disabled.clone()],
                None,
                &Default::default()
            )
            .unwrap(),
            "parent",
        );

        disabled.enabled = true;
        let sibling_duplicate = disabled.clone();
        assert!(resolve_text_with_scopes(
            "{{x}}",
            &[],
            &[],
            &[disabled, sibling_duplicate],
            None,
            &Default::default(),
        )
        .is_err());
    }

    #[test]
    fn rejects_missing_variables_with_actionable_error() {
        let error = resolve_text("{{missing}}", None).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Var);
        assert!(error.message.contains("missing"));
        assert!(!error.hints.is_empty());
    }

    #[test]
    fn rejects_cycles_and_duplicate_names() {
        let cycle = environment(&[("a", "{{b}}"), ("b", "{{a}}")]);
        let error = resolve_text("{{a}}", Some(&cycle)).unwrap_err();
        assert!(error.message.contains("循环引用"));

        let duplicate = Environment {
            id: Uuid::new_v4(),
            name: "重复".into(),
            host: String::new(),
            variables: vec![
                EnvironmentVariable {
                    id: Uuid::new_v4(),
                    key: "x".into(),
                    value: "1".into(),
                    enabled: true,
                    secret: false,
                    has_value: false,
                    clear_secret: false,
                },
                EnvironmentVariable {
                    id: Uuid::new_v4(),
                    key: "x".into(),
                    value: "2".into(),
                    enabled: true,
                    secret: false,
                    has_value: false,
                    clear_secret: false,
                },
            ],
        };
        assert!(resolve_text("{{x}}", Some(&duplicate)).is_err());
    }

    #[test]
    fn resolves_all_request_surfaces_but_ignores_disabled_rows() {
        let mut env = environment(&[("token", "secret")]);
        env.host = "https://example.test".into();
        let spec = RequestSpec {
            method: "POST".into(),
            url: "{{host}}/{{id}}".into(),
            params: vec![KeyValue {
                enabled: true,
                key: "q".into(),
                value: "{{token}}".into(),
                description: None,
                kind: crate::model::ValueKind::Text,
            }],
            path_params: vec![KeyValue {
                enabled: true,
                key: "id".into(),
                value: "42".into(),
                description: None,
                kind: crate::model::ValueKind::Text,
            }],
            headers: vec![KeyValue {
                enabled: false,
                key: "{{missing}}".into(),
                value: "{{missing}}".into(),
                description: None,
                kind: crate::model::ValueKind::Text,
            }],
            body: Body::Json {
                content: "{\"token\":\"{{token}}\"}".into(),
            },
            auth: AuthConfig::Bearer {
                token: "{{token}}".into(),
            },
            settings: Default::default(),
        };

        let resolved = resolve_request(&spec, Some(&env)).unwrap();
        assert_eq!(resolved.url, "https://example.test/{{host}}/{{id}}");
        assert_eq!(resolved.params[0].value, "secret");
        match resolved.body {
            Body::Json { content } => assert_eq!(content, "{\"token\":\"secret\"}"),
            other => panic!("unexpected resolved body: {other:?}"),
        }
        assert_eq!(resolved.headers[0].key, "{{missing}}");
    }

    #[test]
    fn environment_host_resolves_builtin_and_relative_urls() {
        let mut env = environment(&[]);
        env.host = "https://api.example.test".into();

        let mut relative = RequestSpec {
            method: "GET".into(),
            url: "/users".into(),
            params: Vec::new(),
            path_params: Vec::new(),
            headers: Vec::new(),
            body: Body::None,
            auth: AuthConfig::None,
            settings: Default::default(),
        };
        assert_eq!(
            resolve_request(&relative, Some(&env)).unwrap().url,
            "https://api.example.test/users"
        );

        relative.url = "{{host}}/teams".into();
        assert_eq!(
            resolve_request(&relative, Some(&env)).unwrap().url,
            "https://api.example.test/{{host}}/teams"
        );

        relative.url = "https://override.example.test/status".into();
        assert_eq!(
            resolve_request(&relative, Some(&env)).unwrap().url,
            "https://override.example.test/status"
        );
    }

    #[test]
    fn environment_without_host_field_remains_compatible() {
        let environment: Environment = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(),
            "name": "旧环境",
            "variables": []
        }))
        .unwrap();

        assert!(environment.host.is_empty());
    }
}
