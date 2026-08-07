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
    let (normalized, follow_redirects, warnings) = normalize_for_parser(source);
    let parsed = std::panic::catch_unwind(|| ParsedRequest::from_str(&normalized))
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
                follow_redirects: follow_redirects.then_some(true),
                ..Default::default()
            },
        },
        warnings,
    })
}

fn normalize_for_parser(command: &str) -> (String, bool, Vec<String>) {
    let (command, ignored_comments) = normalize_lines(command);
    let command = normalize_executable(&command);
    let command = encode_quotes_in_single_quoted_urls(&command);
    let mut normalized = String::with_capacity(command.len());
    let mut index = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut at_token_start = true;
    let mut follow_redirects = false;

    while index < command.len() {
        let rest = &command[index..];
        let character = rest.chars().next().expect("index is on a char boundary");

        if escaped {
            normalized.push(character);
            escaped = false;
            at_token_start = false;
            index += character.len_utf8();
            continue;
        }

        if quote != Some('\'') && character == '\\' {
            normalized.push(character);
            escaped = true;
            at_token_start = false;
            index += character.len_utf8();
            continue;
        }

        if let Some(active_quote) = quote {
            normalized.push(character);
            if character == active_quote {
                quote = None;
            }
            at_token_start = false;
            index += character.len_utf8();
            continue;
        }

        if character == '\'' || character == '"' {
            quote = Some(character);
            normalized.push(character);
            at_token_start = false;
            index += character.len_utf8();
            continue;
        }

        let location_flag = if at_token_start && rest.starts_with("--location") {
            Some("--location")
        } else if at_token_start && rest.starts_with("-L") {
            Some("-L")
        } else {
            None
        };
        if let Some(flag) = location_flag {
            let suffix = &rest[flag.len()..];
            if suffix.is_empty() || suffix.chars().next().is_some_and(char::is_whitespace) {
                follow_redirects = true;
                index += flag.len();
                continue;
            }
        }

        normalized.push(character);
        at_token_start = character.is_whitespace();
        index += character.len_utf8();
    }

    let warnings = if ignored_comments == 0 {
        Vec::new()
    } else {
        vec![format!("已忽略 {ignored_comments} 行注释")]
    };
    (normalized, follow_redirects, warnings)
}

fn normalize_executable(command: &str) -> String {
    let leading_whitespace = command.len() - command.trim_start().len();
    let start = &command[leading_whitespace..];
    let executable_end = start.find(char::is_whitespace).unwrap_or(start.len());
    let executable = &start[..executable_end];
    if executable.eq_ignore_ascii_case("curl") || executable.eq_ignore_ascii_case("curl.exe") {
        format!(
            "{}curl{}",
            &command[..leading_whitespace],
            &start[executable_end..]
        )
    } else {
        command.to_string()
    }
}

fn encode_quotes_in_single_quoted_urls(command: &str) -> String {
    let mut normalized = String::with_capacity(command.len());
    let mut remainder = command;

    while let Some(start) = remainder.find('\'') {
        normalized.push_str(&remainder[..=start]);
        remainder = &remainder[start + 1..];
        let Some(end) = remainder.find('\'') else {
            normalized.push_str(remainder);
            return normalized;
        };
        let value = &remainder[..end];
        if value.starts_with("http://") || value.starts_with("https://") {
            normalized.push_str(&value.replace('"', "%22"));
        } else {
            normalized.push_str(value);
        }
        normalized.push('\'');
        remainder = &remainder[end + 1..];
    }
    normalized.push_str(remainder);
    normalized
}

/// Accept the line forms emitted by Unix shells, PowerShell and cmd.exe.
/// Comment markers are recognized only when they occupy a complete physical line.
fn normalize_lines(command: &str) -> (String, usize) {
    let mut normalized = String::with_capacity(command.len());
    let mut quote = None;
    let mut ignored_comments = 0;

    for physical_line in command.lines() {
        let trimmed = physical_line.trim_start();
        if quote.is_none() && is_comment_line(trimmed) {
            ignored_comments += 1;
            continue;
        }

        let line = physical_line.trim_end();
        let mut line_quote = quote;
        let mut escaped = false;
        for character in line.chars() {
            if escaped {
                escaped = false;
                continue;
            }
            if line_quote != Some('\'') && character == '\\' {
                escaped = true;
                continue;
            }
            if let Some(active_quote) = line_quote {
                if character == active_quote {
                    line_quote = None;
                }
            } else if character == '\'' || character == '"' {
                line_quote = Some(character);
            }
        }

        let continuation = line_quote.is_none()
            && line
                .chars()
                .last()
                .is_some_and(|character| matches!(character, '\\' | '`' | '^'));
        if continuation {
            let marker = line.chars().last().expect("continuation has a marker");
            normalized.push_str(&line[..line.len() - marker.len_utf8()]);
            normalized.push(' ');
        } else {
            normalized.push_str(line);
            normalized.push(if line_quote.is_some() { '\n' } else { ' ' });
        }
        quote = line_quote;
    }

    (normalized, ignored_comments)
}

fn is_comment_line(line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with("//")
        || line.starts_with("::")
        || line
            .get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("rem"))
            && line
                .get(3..)
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(char::is_whitespace))
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

    #[test]
    fn imports_multiline_get_with_location_and_access_token() {
        let result = import(
            "curl --location  --request GET 'https://business-api.tiktok.com/open_api/v1.3/smart_plus/ad/get/?advertiser_id={{advertiser_id}}&page=1&page_size=100' \\\r\n--header 'Access-Token: {{Access-Token}}'",
        )
        .unwrap();

        assert_eq!(result.spec.method, "GET");
        assert_eq!(
            result.spec.url,
            "https://business-api.tiktok.com/open_api/v1.3/smart_plus/ad/get/"
        );
        assert_eq!(result.spec.params.len(), 3);
        assert_eq!(result.spec.params[0].key, "advertiser_id");
        assert_eq!(result.spec.params[0].value, "{{advertiser_id}}");
        assert_eq!(result.spec.headers[0].key, "access-token");
        assert_eq!(result.spec.headers[0].value, "{{Access-Token}}");
        assert_eq!(result.spec.settings.follow_redirects, Some(true));
    }

    #[test]
    fn imports_documented_command_with_an_inline_comment_line() {
        let result = import(
            r#"curl --location --request GET 'https://business-api.tiktok.com/open_api/v1.3/ad/get/?advertiser_id={{advertiser_id}}&filtering={"campaign_automation_type":"UPGRADED_SMART_PLUS","ad_ids_v2":["{{ad_id_v2}}"]}' \
//input the ID of an Upgraded Smart+ Ad (Asset Group) through {{ad_id_v2}}
--header 'Access-Token: {{Access-Token}}'"#,
        )
        .unwrap();

        assert_eq!(result.spec.method, "GET");
        assert_eq!(result.spec.params.len(), 2);
        assert_eq!(result.spec.params[1].key, "filtering");
        assert_eq!(
            result.spec.params[1].value,
            r#"{"campaign_automation_type":"UPGRADED_SMART_PLUS","ad_ids_v2":["{{ad_id_v2}}"]}"#
        );
        assert_eq!(result.spec.headers[0].value, "{{Access-Token}}");
        assert_eq!(result.warnings, vec!["已忽略 1 行注释"]);
    }

    #[test]
    fn imports_powershell_and_cmd_multiline_commands() {
        for continuation in ['`', '^'] {
            let command = format!(
                "curl --request GET 'https://api.example.test/items' {continuation}\n--header 'Accept: application/json'"
            );
            let result = import(&command).unwrap();
            assert_eq!(result.spec.headers[0].key, "accept");
            assert_eq!(result.spec.headers[0].value, "application/json");
        }
    }

    #[test]
    fn imports_windows_curl_executable_name_case_insensitively() {
        let result = import("CURL.EXE https://api.example.test/items").unwrap();
        assert_eq!(result.spec.url, "https://api.example.test/items");
    }

    #[test]
    fn keeps_comment_markers_inside_quoted_values() {
        let result = import(
            "curl 'https://api.example.test/items' --header 'X-Note: //keep-this' -d '#also-keep-this'",
        )
        .unwrap();

        assert_eq!(result.spec.headers[0].value, "//keep-this");
        assert!(result.warnings.is_empty());
    }
}
