//! Safe, bounded YAML codec shared by workspace persistence and OpenAPI import.
//!
//! Format v1 is unchanged. Parser diagnostics must never retain or print source
//! snippets, scalar values or field names: persisted YAML may contain credentials.
use serde::{de::DeserializeOwned, Serialize};
use std::{fmt, io::Read};

pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct YamlError {
    operation: &'static str,
    location: Option<(u64, u64)>,
}
impl fmt::Display for YamlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "YAML {}失败：检查格式、字段类型及解析限制",
            self.operation
        )?;
        if let Some((line, column)) = self.location {
            write!(f, "（第 {line} 行，第 {column} 列）")?;
        }
        Ok(())
    }
}
impl std::error::Error for YamlError {}

pub fn from_str<T: DeserializeOwned>(input: &str) -> Result<T, YamlError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(YamlError {
            operation: "读取（超过 64 MiB）",
            location: None,
        });
    }
    let mut budget = serde_saphyr::Budget::default();
    // Keep recursive Serde visitors below the Windows worker-thread stack limit.
    budget.max_depth = 32;
    budget.max_documents = 1;
    budget.max_total_scalar_bytes = MAX_INPUT_BYTES;
    let mut options = serde_saphyr::Options::default();
    options.with_snippet = false;
    // Mirror serde_yaml's scalar inference and visitor-driven duplicate handling.
    options.duplicate_keys = serde_saphyr::DuplicateKeyPolicy::PassThrough;
    options.merge_keys = serde_saphyr::MergeKeyPolicy::AsOrdinary;
    options.strict_booleans = true;
    options.leading_zero_numbers_as_strings = true;
    options.underscored_numbers_as_strings = true;
    options.ignore_binary_tag_for_string = true;
    options.non_finite_float_policy = serde_saphyr::NonFiniteFloatPolicy::PassThrough;
    options.budget = Some(budget);
    serde_saphyr::from_str_with_options(input, options).map_err(|error| YamlError {
        operation: "解析",
        location: error
            .location()
            .map(|location| (location.line(), location.column())),
    })
}

pub fn to_string<T: Serialize>(value: &T) -> Result<String, YamlError> {
    let mut options = serde_saphyr::SerializerOptions::default();
    // Unit enums must remain plain scalar strings, not Rust-type YAML tags.
    options.yaml_12 = true;
    options.no_lang_directive = true;
    let output = serde_saphyr::to_string_with_options(value, options).map_err(|_| YamlError {
        operation: "序列化",
        location: None,
    })?;
    if output.len() > MAX_INPUT_BYTES {
        return Err(YamlError {
            operation: "序列化（超过 64 MiB）",
            location: None,
        });
    }
    Ok(output)
}

/// Bound disk reads before allocating an entire oversized workspace file.
pub fn from_reader<T: DeserializeOwned>(reader: impl Read) -> Result<T, YamlError> {
    let mut input = String::new();
    reader
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_string(&mut input)
        .map_err(|_| YamlError {
            operation: "读取（I/O 或 UTF-8 编码）",
            location: None,
        })?;
    from_str(&input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::{json, Value};

    #[test]
    fn legacy_fixtures_preserve_full_models_and_roundtrip_with_lf_crlf_and_bom() {
        fn check<T: Serialize + DeserializeOwned>(fixture: &Value) {
            let original = fixture["yaml"].as_str().unwrap();
            for input in [
                original.to_owned(),
                original.replace('\n', "\r\n"),
                format!("\u{feff}{original}"),
            ] {
                let model: T = from_str(&input).unwrap();
                assert_eq!(serde_json::to_value(&model).unwrap(), fixture["expected"]);
                let output = to_string(&model).unwrap();
                let reread: T = from_str(&output).unwrap();
                assert_eq!(serde_json::to_value(reread).unwrap(), fixture["expected"]);
            }
        }
        let fixtures: Vec<Value> =
            serde_json::from_str(include_str!("../tests/fixtures/yaml-v1-compat.json")).unwrap();
        assert_eq!(fixtures.len(), 41);
        for fixture in fixtures {
            match fixture["kind"].as_str().unwrap() {
                "manifest" => check::<crate::workspace::WorkspaceManifest>(&fixture),
                "environment" => check::<crate::vars::Environment>(&fixture),
                "container" => check::<crate::workspace::ContainerDocument>(&fixture),
                "request" => check::<crate::workspace::PersistedRequest>(&fixture),
                "value" => check::<Value>(&fixture),
                other => panic!("unknown fixture kind {other}"),
            }
        }
    }

    #[test]
    fn properties_and_includes_do_not_expand_implicitly() {
        assert_eq!(from_str::<String>("${HOME}").unwrap(), "${HOME}");
        assert_eq!(
            from_str::<Value>("!include fixture-private-file.yaml").unwrap(),
            "fixture-private-file.yaml"
        );
    }

    #[test]
    fn yaml_12_scalar_inference_aliases_and_literal_merge_keys_are_compatible() {
        let parsed: Value = from_str("yes: yes\nzero: 0755\ncount: 1_000\nflag: True\nbase: &base {v: 1}\ncopy: *base\nmerge: {<<: *base, v: 2}\n").unwrap();
        assert_eq!(parsed["yes"], "yes");
        assert_eq!(parsed["zero"], "0755");
        assert_eq!(parsed["count"], "1_000");
        assert_eq!(parsed["flag"], true);
        assert_eq!(parsed["copy"], json!({"v":1}));
        assert_eq!(parsed["merge"], json!({"<<":{"v":1},"v":2}));
    }

    #[test]
    fn duplicate_keys_keep_visitor_driven_map_and_struct_semantics() {
        let parsed: Value = from_str("key: first\nkey: last\n").unwrap();
        assert_eq!(parsed["key"], "last");
        #[derive(Debug, Deserialize)]
        struct Record {
            #[serde(rename = "key")]
            _key: String,
        }
        assert!(from_str::<Record>("key: first\nkey: last\n").is_err());
    }

    #[test]
    fn parse_and_serializer_errors_never_retain_sources_or_credential_details() {
        #[derive(Debug, Deserialize)]
        struct Record {
            #[serde(rename = "value")]
            _value: u64,
        }
        let failure = from_str::<Record>("value: fixture-private-secret\n").unwrap_err();
        assert!(!format!("{failure:?} {failure}").contains("fixture-private-secret"));
        assert!(std::error::Error::source(&failure).is_none());
        struct Failure;
        impl Serialize for Failure {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("fixture-private-secret"))
            }
        }
        let failure = to_string(&Failure).unwrap_err();
        assert!(!format!("{failure:?} {failure}").contains("fixture-private-secret"));
    }

    #[test]
    fn deep_documents_alias_storms_and_multiple_documents_are_rejected() {
        let nested = format!("{}1{}", "[".repeat(200), "]".repeat(200));
        assert!(from_str::<Value>(&nested).is_err());
        let aliases = format!(
            "base: &base value\nitems: [{}]",
            vec!["*base"; 1000].join(",")
        );
        assert!(from_str::<Value>(&aliases).is_err());
        assert!(from_str::<Value>("---\na: 1\n---\nb: 2\n").is_err());
    }

    #[test]
    fn reader_bounds_input_and_rejects_invalid_utf8_without_echoing_it() {
        let input = std::io::repeat(b'a').take((MAX_INPUT_BYTES + 100) as u64);
        let failure = from_reader::<Value>(input).unwrap_err();
        assert!(failure.to_string().contains("64 MiB"));
        let failure = from_reader::<Value>(&b"fixture-private-secret\xff"[..]).unwrap_err();
        assert!(!format!("{failure:?} {failure}").contains("fixture-private-secret"));
    }

    #[test]
    fn reading_older_minimal_request_and_environment_keeps_defaults() {
        let request: crate::workspace::PersistedRequest = from_str("apogee: 1\nid: 00000000-0000-4000-8000-000000000001\nname: Legacy\nmethod: GET\nurl: https://fixture.test\n").unwrap();
        assert!(matches!(
            request.spec.auth,
            crate::request::AuthConfig::Inherit
        ));
        assert!(request.debug_cases.is_empty());
        assert!(request.spec.path_params.is_empty());
        let environment: crate::vars::Environment = from_str("id: 00000000-0000-4000-8000-000000000001\nname: Legacy\nvariables:\n- key: token\n  value: 00123\n").unwrap();
        assert!(environment.variables[0].enabled);
        assert_eq!(environment.variables[0].value, "00123");
        assert!(environment.host.is_empty());
    }
}
