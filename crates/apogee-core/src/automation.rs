//! v0.3/v0.4 automation primitives shared by the desktop client and CLI.
use crate::request::RequestSpec;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSnapshot {
    pub status: u16,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Assertion {
    Status { expected: u16 },
    BodyContains { value: String },
    JsonEquals { path: String, expected: Value },
    HeaderExists { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionResult {
    pub passed: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Script {
    pub source: String,
    #[serde(default = "default_script_language")]
    pub language: String,
}

fn default_script_language() -> String {
    "javascript".into()
}

/// Small, deterministic pm.variables compatibility surface. Full JS execution can be
/// supplied by a host-specific engine without changing collection data structures.
pub fn apply_script(
    script: &Script,
    variables: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    if !script.language.eq_ignore_ascii_case("javascript") {
        return Err(format!("unsupported script language: {}", script.language));
    }
    let set =
        Regex::new(r#"pm\.variables\.set\(\s*[\"']([^\"']+)[\"']\s*,\s*[\"']([^\"']*)[\"']\s*\)"#)
            .unwrap();
    for c in set.captures_iter(&script.source) {
        variables.insert(c[1].to_owned(), c[2].to_owned());
    }
    let unset = Regex::new(r#"pm\.variables\.unset\(\s*[\"']([^\"']+)[\"']\s*\)"#).unwrap();
    for c in unset.captures_iter(&script.source) {
        variables.remove(&c[1]);
    }
    Ok(())
}

pub fn evaluate_assertion(a: &Assertion, r: &ResponseSnapshot) -> AssertionResult {
    let (passed, message) = match a {
        Assertion::Status { expected } => (
            r.status == *expected,
            format!("status {} (expected {})", r.status, expected),
        ),
        Assertion::BodyContains { value } => {
            (r.body.contains(value), format!("body contains {:?}", value))
        }
        Assertion::JsonEquals { path, expected } => {
            let actual = serde_json::from_str::<Value>(&r.body)
                .ok()
                .and_then(|v| json_path(&v, path).cloned());
            (
                actual.as_ref() == Some(expected),
                format!("JSON {} equals {}", path, expected),
            )
        }
        Assertion::HeaderExists { name } => (
            r.headers.keys().any(|k| k.eq_ignore_ascii_case(name)),
            format!("header {} exists", name),
        ),
    };
    AssertionResult { passed, message }
}

pub fn evaluate_assertions(
    assertions: &[Assertion],
    response: &ResponseSnapshot,
) -> Vec<AssertionResult> {
    assertions
        .iter()
        .map(|a| evaluate_assertion(a, response))
        .collect()
}

/// Supports JSON pointer (`/data/id`) and a compact JSONPath (`$.data.id`, `data.id`).
pub fn json_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() || path == "$" {
        return Some(root);
    }
    let pointer = if path.starts_with('/') {
        path.to_string()
    } else {
        let p = path
            .strip_prefix("$")
            .unwrap_or(path)
            .trim_start_matches('.');
        if p.is_empty() {
            return Some(root);
        }
        format!(
            "/{}",
            p.split('.')
                .map(|s| s.replace('~', "~0").replace('/', "~1"))
                .collect::<Vec<_>>()
                .join("/")
        )
    };
    root.pointer(&pointer)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Extractor {
    JsonPath { name: String, path: String },
    Regex { name: String, pattern: String },
}

pub fn extract_variables(
    extractors: &[Extractor],
    response: &ResponseSnapshot,
) -> Result<BTreeMap<String, String>, String> {
    let json = serde_json::from_str::<Value>(&response.body).ok();
    let mut out = BTreeMap::new();
    for e in extractors {
        match e {
            Extractor::JsonPath { name, path } => {
                let v = json
                    .as_ref()
                    .and_then(|j| json_path(j, path))
                    .ok_or_else(|| format!("JSONPath {} not found", path))?;
                out.insert(
                    name.clone(),
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string()),
                );
            }
            Extractor::Regex { name, pattern } => {
                let re = Regex::new(pattern).map_err(|e| format!("invalid regex: {}", e))?;
                let cap = re
                    .captures(&response.body)
                    .and_then(|c| c.get(1).or_else(|| c.get(0)))
                    .ok_or_else(|| format!("regex {} did not match", pattern))?;
                out.insert(name.clone(), cap.as_str().to_owned());
            }
        }
    }
    Ok(out)
}

pub fn substitute_variables(input: &str, vars: &BTreeMap<String, String>) -> String {
    let re = Regex::new(r"\{\{\s*([A-Za-z0-9_.-]+)\s*\}\}").expect("literal regex");
    re.replace_all(input, |c: &regex::Captures| {
        vars.get(&c[1]).cloned().unwrap_or_else(|| c[0].to_string())
    })
    .into_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerRequest {
    pub name: String,
    pub request: RequestSpec,
    #[serde(default)]
    pub assertions: Vec<Assertion>,
    #[serde(default)]
    pub extractors: Vec<Extractor>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerResult {
    pub name: String,
    pub response: ResponseSnapshot,
    pub assertions: Vec<AssertionResult>,
    pub variables: BTreeMap<String, String>,
}

pub trait RequestExecutor {
    fn execute(
        &mut self,
        request: &RequestSpec,
        variables: &BTreeMap<String, String>,
    ) -> Result<ResponseSnapshot, String>;
}

/// Runs requests in declaration order; extracted variables are available to subsequent requests.
pub fn run_collection<E: RequestExecutor>(
    requests: &[RunnerRequest],
    iterations: usize,
    initial: BTreeMap<String, String>,
    executor: &mut E,
) -> Result<Vec<RunnerResult>, String> {
    let mut all = Vec::new();
    for _ in 0..iterations.max(1) {
        let mut vars = initial.clone();
        for item in requests {
            let mut req = item.request.clone();
            req.url = substitute_variables(&req.url, &vars);
            for h in &mut req.headers {
                h.key = substitute_variables(&h.key, &vars);
                h.value = substitute_variables(&h.value, &vars);
            }
            let response = executor.execute(&req, &vars)?;
            let assertions = evaluate_assertions(&item.assertions, &response);
            let extracted = extract_variables(&item.extractors, &response)?;
            vars.extend(extracted.clone());
            all.push(RunnerResult {
                name: item.name.clone(),
                response,
                assertions,
                variables: extracted,
            });
        }
    }
    Ok(all)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedApi {
    pub name: String,
    pub requests: Vec<(String, RequestSpec)>,
    pub warnings: Vec<String>,
}

/// Imports OpenAPI 3.x (JSON or YAML), creating one request per operation.
pub fn import_openapi(input: &str) -> Result<ImportedApi, String> {
    let root: Value = serde_json::from_str(input)
        .or_else(|_| serde_yaml_ng::from_str(input))
        .map_err(|e| format!("invalid OpenAPI: {}", e))?;
    let name = root
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or("OpenAPI")
        .to_owned();
    let mut requests = Vec::new();
    let mut warnings = Vec::new();
    let schemes = root
        .pointer("/servers/0/url")
        .and_then(Value::as_str)
        .unwrap_or("http://localhost");
    if let Some(paths) = root.get("paths").and_then(Value::as_object) {
        for (path, item) in paths {
            if let Some(obj) = item.as_object() {
                for (method, op) in obj {
                    if !matches!(
                        method.as_str(),
                        "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
                    ) {
                        continue;
                    }
                    let operation = op.as_object();
                    let title = operation
                        .and_then(|o| o.get("summary").or_else(|| o.get("operationId")))
                        .and_then(Value::as_str)
                        .unwrap_or(method)
                        .to_owned();
                    let path_params = path
                        .split('{')
                        .skip(1)
                        .filter_map(|part| part.split('}').next())
                        .map(|name| crate::model::KeyValue {
                            enabled: true,
                            key: name.to_owned(),
                            value: format!("{{{{{}}}}}", name),
                            description: None,
                            kind: crate::model::ValueKind::Text,
                        })
                        .collect();
                    requests.push((
                        title,
                        RequestSpec {
                            method: method.to_uppercase(),
                            url: format!("{}{}", schemes.trim_end_matches('/'), path),
                            params: Vec::new(),
                            path_params,
                            headers: Vec::new(),
                            body: Default::default(),
                            auth: Default::default(),
                            settings: Default::default(),
                        },
                    ));
                }
            }
        }
    } else {
        warnings.push("OpenAPI 文档缺少 paths".into());
    }
    Ok(ImportedApi {
        name,
        requests,
        warnings,
    })
}

pub fn generate_code(request: &RequestSpec, language: &str) -> Result<String, String> {
    let url = request.url.replace('"', "\\\"");
    let method = request.method.to_uppercase();
    match language.to_lowercase().as_str() {
        "curl" => Ok(format!("curl -X {} \"{}\"", method, url)),
        "fetch" | "javascript" => Ok(format!("fetch(\"{}\", {{ method: \"{}\" }})", url, method)),
        "python" => Ok(format!("import requests\nrequests.request(\"{}\", \"{}\")", method, url)),
        "go" => Ok(format!("http.NewRequest(\"{}\", \"{}\", nil)", method, url)),
        "java" => Ok(format!("HttpRequest.newBuilder(URI.create(\"{}\")).method(\"{}\", HttpRequest.BodyPublishers.noBody()).build()", url, method)),
        "rust" => Ok(format!("reqwest::Client::new().request(reqwest::Method::{} , \"{}\")", method, url)),
        other => Err(format!("unsupported language: {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_and_extract() {
        let r = ResponseSnapshot {
            status: 200,
            body: r#"{"data":{"id":7}}"#.into(),
            ..Default::default()
        };
        assert_eq!(
            json_path(&serde_json::from_str(&r.body).unwrap(), "$.data.id").unwrap(),
            &Value::from(7)
        );
        let x = extract_variables(
            &[Extractor::JsonPath {
                name: "id".into(),
                path: "$.data.id".into(),
            }],
            &r,
        )
        .unwrap();
        assert_eq!(x["id"], "7");
    }
    #[test]
    fn assertion_and_substitution() {
        let r = ResponseSnapshot {
            status: 201,
            ..Default::default()
        };
        assert!(evaluate_assertion(&Assertion::Status { expected: 201 }, &r).passed);
        let mut v = BTreeMap::new();
        v.insert("id".into(), "42".into());
        assert_eq!(substitute_variables("/x/{{id}}", &v), "/x/42");
    }
}
