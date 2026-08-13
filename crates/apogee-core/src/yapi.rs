//! YApi 项目的一次性导入。
//!
//! 网络访问和字段转换都在 core 中完成；前端只接收摘要和不可预测的 plan id。

use crate::error::{AppError, ErrorKind, Hint};
use crate::model::{KeyValue, ValueKind};
use crate::postman::ImportedRequest;
use crate::request::{AuthConfig, Body, RequestSettings, RequestSpec};
use crate::vars::{Environment, EnvironmentVariable};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct YapiImportPlan {
    pub project_name: String,
    pub requests: Vec<ImportedRequest>,
    pub environments: Vec<Environment>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YapiCategorySummary {
    pub name: String,
    pub request_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YapiAnalyzeResult {
    pub plan_id: Uuid,
    pub project_name: String,
    pub categories: Vec<YapiCategorySummary>,
    pub request_count: usize,
    pub environment_count: usize,
    pub warnings: Vec<String>,
}

impl YapiImportPlan {
    pub fn summary(&self, plan_id: Uuid) -> YapiAnalyzeResult {
        let mut categories = Vec::<YapiCategorySummary>::new();
        for request in &self.requests {
            let name = request
                .folder
                .first()
                .cloned()
                .unwrap_or_else(|| "未分类".into());
            if let Some(category) = categories.iter_mut().find(|item| item.name == name) {
                category.request_count += 1;
            } else {
                categories.push(YapiCategorySummary {
                    name,
                    request_count: 1,
                });
            }
        }
        YapiAnalyzeResult {
            plan_id,
            project_name: self.project_name.clone(),
            request_count: self.requests.len(),
            environment_count: self.environments.len(),
            categories,
            warnings: self.warnings.clone(),
        }
    }
}

pub async fn analyze(base_url: &str, token: &str) -> Result<YapiImportPlan, AppError> {
    let base = normalize_base_url(base_url)?;
    let token = token.trim();
    if token.is_empty() {
        return Err(AppError::new(ErrorKind::Parse, "项目 Token 不能为空")
            .hint(Hint::new("在 YApi 项目设置的 Token 配置中复制项目 Token")));
    }

    ensure_crypto_provider();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|_| yapi_error(ErrorKind::Connect, "无法初始化 YApi 连接"))?;

    let project = get_api(&client, &base, "api/project/get", token, &[]).await?;
    let project_name = text(&project, "name").unwrap_or("YApi 项目").to_string();
    let basepath = text(&project, "basepath").unwrap_or_default();
    let environments = parse_environments(project.get("env"));
    let mut warnings = project_warnings(&project);

    let exported = get_api(
        &client,
        &base,
        "api/plugin/export",
        token,
        &[("type", "json"), ("status", "all")],
    )
    .await;

    let categories = match exported {
        Ok(value) => export_categories(&value),
        Err(_) => {
            warnings.push("当前 YApi 未提供 JSON 导出接口，已使用接口详情回退链路".into());
            fallback_categories(&client, &base, token).await?
        }
    };

    let mut requests = Vec::new();
    for (category, interfaces) in categories {
        for interface in interfaces {
            if let Some(request) = convert_interface(&interface, &category, basepath, &mut warnings)
            {
                requests.push(request);
            }
        }
    }
    if requests.is_empty() {
        warnings.push("项目中没有可导入的接口".into());
    }
    deduplicate_warnings(&mut warnings);

    Ok(YapiImportPlan {
        project_name,
        requests,
        environments,
        warnings,
    })
}

async fn fallback_categories(
    client: &reqwest::Client,
    base: &Url,
    token: &str,
) -> Result<Vec<(String, Vec<Value>)>, AppError> {
    let menu = get_api(client, base, "api/interface/list_menu", token, &[]).await?;
    let categories = menu.as_array().cloned().unwrap_or_default();
    let mut result = Vec::new();
    for category in categories {
        let name = text(&category, "name").unwrap_or("未分类").to_string();
        let ids: Vec<String> = category
            .get("list")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("_id").and_then(value_as_string))
            .collect();
        let mut interfaces = Vec::with_capacity(ids.len());
        let mut tasks = tokio::task::JoinSet::new();
        for id in ids {
            while tasks.len() >= 8 {
                let detail = tasks
                    .join_next()
                    .await
                    .expect("YApi detail task set is not empty")
                    .map_err(|_| yapi_error(ErrorKind::Http, "读取 YApi 接口详情失败"))??;
                interfaces.push(detail);
            }
            let client = client.clone();
            let base = base.clone();
            let token = token.to_string();
            tasks.spawn(async move {
                get_api(&client, &base, "api/interface/get", &token, &[("id", &id)]).await
            });
        }
        while let Some(detail) = tasks.join_next().await {
            interfaces
                .push(detail.map_err(|_| yapi_error(ErrorKind::Http, "读取 YApi 接口详情失败"))??);
        }
        result.push((name, interfaces));
    }
    Ok(result)
}

async fn get_api(
    client: &reqwest::Client,
    base: &Url,
    path: &str,
    token: &str,
    query: &[(&str, &str)],
) -> Result<Value, AppError> {
    let mut url = base
        .join(path)
        .map_err(|_| yapi_error(ErrorKind::Parse, "YApi 地址无效"))?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("token", token);
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    let response = client.get(url).send().await.map_err(|error| {
        let description = error.to_string().to_ascii_lowercase();
        if description.contains("certificate")
            || description.contains("tls")
            || description.contains("ssl")
        {
            return AppError::new(ErrorKind::Tls, "YApi TLS 证书校验失败")
                .hint(Hint::new(
                    "检查实例证书是否过期，以及系统是否信任其签发机构",
                ))
                .hint(Hint::new("内网自签证书请先安装到系统受信任根证书存储"));
        }
        if error.is_timeout() {
            yapi_error(ErrorKind::Timeout, "连接 YApi 超时")
        } else if description.contains("dns") {
            yapi_error(ErrorKind::Dns, "无法解析 YApi 主机名")
        } else if error.is_connect() {
            yapi_error(ErrorKind::Connect, "无法连接 YApi，请检查实例地址和网络")
        } else {
            yapi_error(ErrorKind::Http, "YApi 网络请求失败")
        }
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(yapi_error(
            ErrorKind::Http,
            format!("YApi 返回 HTTP {}", status.as_u16()),
        ));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| yapi_error(ErrorKind::Http, "读取 YApi 响应失败"))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| yapi_error(ErrorKind::Parse, "YApi 返回了无法解析的 JSON"))?;
    unwrap_envelope(value)
}

fn unwrap_envelope(value: Value) -> Result<Value, AppError> {
    if let Some(code) = value.get("errcode").and_then(Value::as_i64) {
        if code != 0 {
            let message = value
                .get("errmsg")
                .and_then(Value::as_str)
                .unwrap_or("Token 无效或项目不可访问");
            return Err(yapi_error(ErrorKind::Http, message));
        }
        return Ok(value.get("data").cloned().unwrap_or(Value::Null));
    }
    Ok(value)
}

fn export_categories(value: &Value) -> Vec<(String, Vec<Value>)> {
    let root = value.get("data").unwrap_or(value);
    root.as_array()
        .into_iter()
        .flatten()
        .map(|category| {
            let name = text(category, "name").unwrap_or("未分类").to_string();
            let list = category
                .get("list")
                .or_else(|| category.get("interface"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            (name, list)
        })
        .collect()
}

fn convert_interface(
    value: &Value,
    category: &str,
    basepath: &str,
    warnings: &mut Vec<String>,
) -> Option<ImportedRequest> {
    let name = text(value, "title")
        .or_else(|| text(value, "name"))
        .unwrap_or("未命名接口")
        .trim()
        .to_string();
    let path = text(value, "path").unwrap_or_default().trim();
    if path.is_empty() {
        warnings.push(format!("接口“{name}”缺少路径，已跳过"));
        return None;
    }

    let headers = rows(value.get("req_headers"), false);
    let params = rows(value.get("req_query"), false);
    let path_params = rows(value.get("req_params"), false);
    let body = convert_body(value, &headers, &name, warnings);

    if text(value, "markdown").is_some_and(|item| !item.trim().is_empty())
        || text(value, "desc").is_some_and(|item| !item.trim().is_empty())
    {
        warnings.push("接口描述/Markdown 暂不支持，未写入请求".into());
    }
    if value.get("res_body").is_some() || value.get("res_body_type").is_some() {
        warnings.push("响应示例和响应 Schema 暂不支持，未写入请求".into());
    }

    Some(ImportedRequest {
        name,
        folder: vec![if category.trim().is_empty() {
            "未分类".into()
        } else {
            category.to_string()
        }],
        spec: RequestSpec {
            method: text(value, "method").unwrap_or("GET").to_uppercase(),
            url: join_relative_path(basepath, path),
            params,
            path_params,
            headers,
            body,
            auth: AuthConfig::None,
            settings: RequestSettings::default(),
        },
    })
}

fn convert_body(
    value: &Value,
    headers: &[KeyValue],
    name: &str,
    warnings: &mut Vec<String>,
) -> Body {
    match text(value, "req_body_type")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "form" => {
            let rows = rows(value.get("req_body_form"), true);
            let has_file = rows.iter().any(|row| row.kind == ValueKind::File);
            if has_file {
                warnings.push(format!(
                    "接口“{name}”包含文件参数，导入后需重新选择本地文件"
                ));
                Body::FormData { rows }
            } else {
                let urlencoded = headers.iter().any(|header| {
                    header.key.eq_ignore_ascii_case("content-type")
                        && header
                            .value
                            .to_ascii_lowercase()
                            .contains("application/x-www-form-urlencoded")
                });
                if urlencoded {
                    Body::UrlEncoded { rows }
                } else {
                    Body::FormData { rows }
                }
            }
        }
        "json" => {
            let raw = text(value, "req_body_other").unwrap_or_default();
            if value
                .get("req_body_is_json_schema")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                warnings.push(format!("接口“{name}”的 JSON Schema 已转换为示例 JSON"));
                let content = serde_json::from_str::<Value>(raw)
                    .ok()
                    .map(|schema| schema_example(&schema))
                    .and_then(|example| serde_json::to_string_pretty(&example).ok())
                    .unwrap_or_else(|| "{}".into());
                Body::Json { content }
            } else {
                Body::Json {
                    content: raw.to_string(),
                }
            }
        }
        "raw" | "text" => Body::Text {
            content: text(value, "req_body_other")
                .unwrap_or_default()
                .to_string(),
            content_type: headers
                .iter()
                .find(|header| header.key.eq_ignore_ascii_case("content-type"))
                .map(|header| header.value.clone())
                .unwrap_or_else(|| "text/plain".into()),
        },
        "file" => {
            warnings.push(format!(
                "接口“{name}”的二进制文件路径已留空，请重新选择本地文件"
            ));
            Body::Binary {
                path: Default::default(),
            }
        }
        _ => Body::None,
    }
}

fn rows(value: Option<&Value>, files: bool) -> Vec<KeyValue> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let key = text(item, "name").or_else(|| text(item, "key"))?.trim();
            if key.is_empty() {
                return None;
            }
            let is_file =
                files && text(item, "type").is_some_and(|kind| kind.eq_ignore_ascii_case("file"));
            Some(KeyValue {
                enabled: true,
                key: key.to_string(),
                value: if is_file {
                    String::new()
                } else {
                    text(item, "value")
                        .filter(|item| !item.is_empty())
                        .or_else(|| text(item, "example"))
                        .unwrap_or_default()
                        .to_string()
                },
                description: text(item, "desc")
                    .or_else(|| text(item, "description"))
                    .filter(|item| !item.is_empty())
                    .map(str::to_string),
                kind: if is_file {
                    ValueKind::File
                } else {
                    ValueKind::Text
                },
            })
        })
        .collect()
}

fn parse_environments(value: Option<&Value>) -> Vec<Environment> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|environment| Environment {
            id: Uuid::new_v4(),
            name: text(environment, "name").unwrap_or("YApi 环境").to_string(),
            host: text(environment, "domain")
                .unwrap_or_default()
                .trim_end_matches('/')
                .to_string(),
            variables: environment
                .get("global")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|variable| {
                    let key = text(variable, "name")?.trim();
                    (!key.is_empty()).then(|| EnvironmentVariable {
                        key: key.to_string(),
                        value: text(variable, "value").unwrap_or_default().to_string(),
                        enabled: true,
                    })
                })
                .collect(),
        })
        .collect()
}

fn project_warnings(project: &Value) -> Vec<String> {
    let mut warnings = Vec::new();
    if project
        .get("pre_script")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || project
            .get("after_script")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    {
        warnings.push("项目前置/后置脚本暂不支持，未导入".into());
    }
    if project
        .get("env")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|env| {
            env.get("header")
                .and_then(Value::as_array)
                .is_some_and(|rows| !rows.is_empty())
        })
    {
        warnings.push("环境级公共 Header 暂不支持，未导入".into());
    }
    warnings
}

fn schema_example(schema: &Value) -> Value {
    if let Some(example) = schema.get("example") {
        return example.clone();
    }
    if let Some(default) = schema.get("default") {
        return default.clone();
    }
    match text(schema, "type").unwrap_or_default() {
        "object" => Value::Object(
            schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| {
                    properties
                        .iter()
                        .map(|(key, value)| (key.clone(), schema_example(value)))
                        .collect()
                })
                .unwrap_or_default(),
        ),
        "array" => Value::Array(vec![schema_example(
            schema.get("items").unwrap_or(&Value::Null),
        )]),
        "integer" | "number" => Value::from(0),
        "boolean" => Value::Bool(false),
        _ => Value::String(String::new()),
    }
}

fn normalize_base_url(input: &str) -> Result<Url, AppError> {
    let value = input.trim();
    let mut url = Url::parse(value).map_err(|_| {
        AppError::new(ErrorKind::Parse, "YApi 地址无效")
            .hint(Hint::new("请输入实例地址，例如 https://yapi.example.com"))
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(
            AppError::new(ErrorKind::Parse, "YApi 地址必须是 HTTP 或 HTTPS 地址")
                .hint(Hint::new("请输入 YApi 实例地址或项目页面地址")),
        );
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path();
    let root_path = path
        .find("/project/")
        .map(|index| &path[..index])
        .unwrap_or(path);
    let normalized = format!("{}/", root_path.trim_end_matches('/'));
    url.set_path(&normalized);
    Ok(url)
}

fn join_relative_path(basepath: &str, path: &str) -> String {
    let base = basepath.trim().trim_matches('/');
    let endpoint = path.trim();
    if base.is_empty() {
        format!("/{}", endpoint.trim_start_matches('/'))
    } else {
        format!("/{base}/{}", endpoint.trim_start_matches('/'))
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn value_as_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
}

fn deduplicate_warnings(warnings: &mut Vec<String>) {
    let mut seen = HashSet::new();
    warnings.retain(|warning| seen.insert(warning.clone()));
}

fn yapi_error(kind: ErrorKind, message: impl Into<String>) -> AppError {
    AppError::new(kind, message).hint(Hint::new("检查实例地址、项目 Token 和网络后重试"))
}

fn ensure_crypto_provider() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
