//! 请求变量与环境解析。
//!
//! 环境值只在发送前解析，不写回 `RequestSpec`。这样请求 YAML 仍保存
//! `{{variable}}` 模板，切换环境不会污染请求草稿，也不会把某个环境的
//! 实际值意外提交到 Git。

use crate::error::{AppError, ErrorKind, Hint};
use crate::model::KeyValue;
use crate::request::{AuthConfig, Body, RequestSpec};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    pub key: String,
    #[serde(default)]
    pub value: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

const MAX_RESOLUTION_DEPTH: usize = 16;

/// 用一个环境解析请求模板中的全部 `{{name}}` 占位符。
///
/// 解析发生在 HTTP 请求建造之前，因此 URL、query、header、body 和鉴权看到的
/// 是同一份环境快照。未定义、循环引用和超过深度的变量都返回 `ErrorKind::Var`，
/// 不会把原始占位符静默发送到网络。
pub fn resolve_request(
    spec: &RequestSpec,
    environment: Option<&Environment>,
) -> Result<RequestSpec, AppError> {
    let resolver = Resolver::from_environment(environment)?;
    let mut resolved = spec.clone();

    resolved.url = resolver.resolve_text(&spec.url)?;
    if let Some(environment) = environment {
        resolved.url = with_environment_host(&resolved.url, &environment.host);
    }
    resolved.params = resolver.resolve_rows(&spec.params)?;
    resolved.path_params = resolver.resolve_rows(&spec.path_params)?;
    resolved.headers = resolver.resolve_rows(&spec.headers)?;
    resolved.body = resolver.resolve_body(&spec.body)?;
    resolved.auth = resolver.resolve_auth(&spec.auth)?;

    Ok(resolved)
}

/// 对单段文本做变量解析，供未来的预览命令复用。
pub fn resolve_text(text: &str, environment: Option<&Environment>) -> Result<String, AppError> {
    Resolver::from_environment(environment)?.resolve_text(text)
}

struct Resolver {
    values: HashMap<String, String>,
}

impl Resolver {
    fn from_environment(environment: Option<&Environment>) -> Result<Self, AppError> {
        let mut values = HashMap::new();
        if let Some(environment) = environment {
            for variable in environment.variables.iter().filter(|item| item.enabled) {
                let key = variable.key.trim();
                if key.is_empty() {
                    continue;
                }
                if values.insert(key.to_string(), variable.value.clone()).is_some() {
                    return Err(AppError::new(
                        ErrorKind::Var,
                        format!("环境变量名重复：{key}"),
                    )
                    .detail(format!("环境「{}」中有多个同名变量。", environment.name))
                    .hint(Hint::new("删除重复变量，或为变量改名后再发送")));
                }
            }
            let host = environment.host.trim().trim_end_matches('/');
            if !host.is_empty() {
                values.insert("host".to_string(), host.to_string());
            }
        }
        Ok(Self { values })
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
                    .detail(format!("从第 {} 个字符开始的 `{{{{` 没有对应的 `}}}}`。", start + 1))
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

        let raw = self.values.get(name).cloned().ok_or_else(|| {
            AppError::new(ErrorKind::Var, format!("变量未定义：{name}"))
                .detail("请求中引用了当前环境不存在或已禁用的变量。")
                .hint(Hint::new("选择包含该变量的环境，或在环境变量面板中补充值"))
        })?;

        stack.push(name.to_string());
        let result = self.resolve_text_inner(&raw, stack, depth + 1);
        stack.pop();
        result
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
    if url.is_empty()
        || host.is_empty()
        || url::Url::parse(url).is_ok()
    {
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
                    key: (*key).into(),
                    value: (*value).into(),
                    enabled: true,
                })
                .collect(),
        }
    }

    #[test]
    fn resolves_nested_variables() {
        let env = environment(&[("host", "api.example.test"), ("base", "https://{{host}}/v1")]);
        assert_eq!(resolve_text("{{base}}/users", Some(&env)).unwrap(), "https://api.example.test/v1/users");
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
                EnvironmentVariable { key: "x".into(), value: "1".into(), enabled: true },
                EnvironmentVariable { key: "x".into(), value: "2".into(), enabled: true },
            ],
        };
        assert!(resolve_text("{{x}}", Some(&duplicate)).is_err());
    }

    #[test]
    fn resolves_all_request_surfaces_but_ignores_disabled_rows() {
        let env = environment(&[("host", "https://example.test"), ("token", "secret")]);
        let spec = RequestSpec {
            method: "POST".into(),
            url: "{{host}}/:id".into(),
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
            body: Body::Json { content: "{\"token\":\"{{token}}\"}".into() },
            auth: AuthConfig::Bearer { token: "{{token}}".into() },
            settings: Default::default(),
        };

        let resolved = resolve_request(&spec, Some(&env)).unwrap();
        assert_eq!(resolved.url, "https://example.test/:id");
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
            "https://api.example.test/teams"
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
