//! Explicit, portable export. This projection contains credentials; never use for history.
use crate::{
    model::{KeyValue, ValueKind},
    request::{AuthConfig, Body, RequestSpec},
};
use serde_json::{json, Value};

pub fn auth(auth: &AuthConfig) -> Option<Value> {
    let (kind, fields) = match auth {
        AuthConfig::Inherit => return None,
        AuthConfig::None => return Some(json!({"type":"noauth"})),
        AuthConfig::Basic { username, password } => {
            ("basic", json!({"username":username,"password":password}))
        }
        AuthConfig::Bearer { token } => ("bearer", json!({"token":token})),
        AuthConfig::ApiKey {
            key,
            value,
            location,
        } => ("apikey", json!({"key":key,"value":value,"in":location})),
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
        } => (
            "oauth1",
            json!({"consumerKey":consumer_key,"consumerSecret":consumer_secret,"token":token,"tokenSecret":token_secret,
                "signatureMethod":signature_method,"realm":realm,"callback":callback,"verifier":verifier,"nonce":nonce,"timestamp":timestamp,
                "addParamsToHeader":"true","addEmptyParamsToSign":"false"}),
        ),
        AuthConfig::OAuth2 {
            access_token,
            header_prefix,
            token_request,
            ..
        } => {
            let mut fields = json!({"accessToken":access_token,"headerPrefix":header_prefix,"addTokenTo":"header"});
            if let Some(config) = token_request {
                let pkce = config.grant_type == crate::oauth2::OAuth2Flow::AuthorizationCode;
                let extra = json!({"grant_type":if pkce {"authorization_code_with_pkce"} else {"client_credentials"},
                    "accessTokenUrl":config.token_url,"clientId":config.client_id,"clientSecret":config.client_secret,"scope":config.scope,
                    "refreshToken":config.refresh_token,"client_authentication":if config.client_auth == crate::oauth2::OAuth2ClientAuth::Basic {"header"} else {"body"}});
                fields
                    .as_object_mut()
                    .unwrap()
                    .extend(extra.as_object().unwrap().clone());
                if pkce {
                    fields.as_object_mut().unwrap().extend(json!({"authUrl":config.authorization_url,"redirect_uri":config.redirect_uri,"code_challenge_method":"S256"}).as_object().unwrap().clone());
                }
            }
            ("oauth2", fields)
        }
        AuthConfig::Jwt {
            secret,
            payload,
            header,
            algorithm,
            header_prefix,
        } => (
            "jwt",
            json!({"secret":secret,"payload":payload,"header":header,
            "algorithm":algorithm,"headerPrefix":header_prefix,"isSecretBase64Encoded":"false","addTokenTo":"header"}),
        ),
    };
    let values: Vec<Value> = fields
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| json!({"key":key,"value":value,"type":"string"}))
        .collect();
    Some(json!({"type":kind,kind:values}))
}

fn rows(values: &[KeyValue], form: bool) -> Vec<Value> {
    values
        .iter()
        .map(|row| {
            let mut value = json!({"key":row.key,"disabled":!row.enabled});
            if form && row.kind == ValueKind::File {
                value["type"] = json!("file");
                value["src"] = json!(row.value);
            } else {
                value["value"] = json!(row.value);
                if form {
                    value["type"] = json!("text");
                }
            }
            if let Some(description) = &row.description {
                value["description"] = json!(description);
            }
            value
        })
        .collect()
}

fn body(body: &Body) -> Option<Value> {
    Some(match body {
        Body::None => return None,
        Body::Json { content } => {
            json!({"mode":"raw","raw":content,"options":{"raw":{"language":"json"}}})
        }
        Body::Text { content, .. } => {
            json!({"mode":"raw","raw":content,"options":{"raw":{"language":"text"}}})
        }
        Body::FormData { rows: values } => json!({"mode":"formdata","formdata":rows(values,true)}),
        Body::UrlEncoded { rows: values } => {
            json!({"mode":"urlencoded","urlencoded":rows(values,false)})
        }
        Body::Binary { path } => json!({"mode":"file","file":{"src":path}}),
        Body::GraphQl {
            query,
            variables,
            operation_name,
        } => {
            json!({"mode":"graphql","graphql":{"query":query,"variables":variables,"operationName":operation_name}})
        }
    })
}

fn portable_url(spec: &RequestSpec) -> String {
    let end = spec.url.find(['?', '#']).unwrap_or(spec.url.len());
    let (address, suffix) = spec.url.split_at(end);
    let authority = address.find("://").map_or(0, |i| i + 3);
    let path_start = if address.starts_with('/') {
        Some(0)
    } else {
        address[authority..].find('/').map(|i| i + authority)
    };
    let mut url = address.to_string();
    if let Some(start) = path_start {
        let mut path = address[start..].to_string();
        for row in &spec.path_params {
            path = path.replace(&format!("{{{{{}}}}}", row.key), &format!(":{}", row.key));
            path = path.replace(&format!("{{{}}}", row.key), &format!(":{}", row.key));
        }
        url = format!("{}{path}", &address[..start]);
    }
    url.push_str(suffix);
    // Only explicit URL environment tokens are reversed; legacy path tokens stay paths.
    url.replace("{{env:", "{{")
}

fn query_piece(value: &str) -> String {
    static TEMPLATE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = TEMPLATE
        .get_or_init(|| regex::Regex::new(r"\{\{[^{}]+\}\}").expect("valid template pattern"));
    let mut result = String::new();
    let mut cursor = 0;
    for token in pattern.find_iter(value) {
        result.extend(url::form_urlencoded::byte_serialize(
            &value.as_bytes()[cursor..token.start()],
        ));
        result.push_str(token.as_str());
        cursor = token.end();
    }
    result.extend(url::form_urlencoded::byte_serialize(
        &value.as_bytes()[cursor..],
    ));
    result
}

pub fn request(spec: &RequestSpec) -> Value {
    let mut headers = rows(&spec.headers, false);
    if let Body::Text { content_type, .. } = &spec.body {
        if !spec
            .headers
            .iter()
            .any(|row| row.enabled && row.key.eq_ignore_ascii_case("content-type"))
        {
            headers.push(json!({"key":"Content-Type","value":content_type,"disabled":false}));
        }
    }
    let raw = portable_url(spec);
    let mut url = json!({"raw":raw});
    if !spec.params.is_empty() {
        let (address, fragment) = raw
            .split_once('#')
            .map_or((raw.as_str(), None), |(a, f)| (a, Some(f)));
        let (base, query) = address
            .split_once('?')
            .map_or((address, ""), |(a, q)| (a, q));
        let mut values: Vec<KeyValue> = url::form_urlencoded::parse(query.as_bytes())
            .map(|(key, value)| KeyValue {
                key: key.into_owned(),
                value: value.into_owned(),
                enabled: true,
                description: None,
                kind: ValueKind::Text,
            })
            .collect();
        values.extend(spec.params.clone());
        let query = values
            .iter()
            .filter(|row| row.enabled && !row.key.is_empty())
            .map(|row| format!("{}={}", query_piece(&row.key), query_piece(&row.value)))
            .collect::<Vec<_>>()
            .join("&");
        let mut consistent = base.to_string();
        if !query.is_empty() {
            consistent.push('?');
            consistent.push_str(&query);
        }
        if let Some(fragment) = fragment {
            consistent.push('#');
            consistent.push_str(fragment);
        }
        url["raw"] = json!(consistent);
        url["query"] = json!(rows(&values, false));
    }
    if !spec.path_params.is_empty() {
        url["variable"] = json!(rows(&spec.path_params, false));
    }
    let mut result = json!({"method":spec.method,"header":headers,"url":url});
    if let Some(body) = body(&spec.body) {
        result["body"] = body;
    }
    if let Some(auth) = auth(&spec.auth) {
        result["auth"] = auth;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_all_auth_configuration_including_pkce_without_ephemeral_or_lifecycle_values() {
        for config in [
            json!({"type":"none"}),
            json!({"type":"basic","username":"u","password":"fixture"}),
            json!({"type":"bearer","token":"fixture"}),
            json!({"type":"apiKey","key":"sid","value":"fixture","location":"cookie"}),
            json!({"type":"oauth1","consumerKey":"key","consumerSecret":"fixture","signatureMethod":"HMAC-SHA256"}),
            json!({"type":"jwt","secret":"fixture","payload":"{}","header":"{}","algorithm":"HS384","headerPrefix":"Bearer"}),
            json!({"type":"oauth2","accessToken":"fixture","headerPrefix":"Token","expiresAt":123,"tokenContext":"private-context","tokenRequest":{
                "grantType":"authorizationCode","authorizationUrl":"https://provider.test/authorize","redirectUri":"http://127.0.0.1:53682/callback","tokenUrl":"https://provider.test/token",
                "clientId":"id","clientSecret":"fixture-client","scope":"read","refreshToken":"fixture-refresh","clientAuth":"body"}}),
        ] {
            let value: AuthConfig = serde_json::from_value(config).unwrap();
            let exported = auth(&value).unwrap();
            let imported = super::super::parse_auth(&exported).unwrap();
            assert_eq!(
                std::mem::discriminant(&value),
                std::mem::discriminant(&imported)
            );
            let text = exported.to_string();
            assert!(!text.contains("private-context"));
            assert!(!text.contains("expiresAt"));
            assert!(!text.contains("code_verifier"));
            if let AuthConfig::OAuth2 {
                token_request: Some(config),
                ..
            } = imported
            {
                assert_eq!(config.client_secret, "fixture-client");
                assert_eq!(config.authorization_url, "https://provider.test/authorize");
                assert_eq!(
                    config.grant_type,
                    crate::oauth2::OAuth2Flow::AuthorizationCode
                );
            }
        }
        assert!(auth(&AuthConfig::Inherit).is_none());
    }
    #[test]
    fn mixed_query_roundtrip_preserves_order_duplicates_disabled_and_templates() {
        let source: RequestSpec = serde_json::from_value(json!({"method":"GET","url":"https://test.local/x?from=raw&x=1&x=2&v={{env:q}}#part", "params":[{"key":"extra","value":"table","enabled":true},{"key":"off","value":"secret","enabled":false}]})).unwrap();
        let exported = request(&source);
        assert!(exported["url"]["raw"].as_str().unwrap().contains("v={{q}}"));
        assert_eq!(exported["url"]["query"].as_array().unwrap().len(), 6);
        let imported =
            super::super::import_json(&json!({"item":[{"request":exported}]}).to_string()).unwrap();
        let env = crate::vars::Environment {
            id: uuid::Uuid::new_v4(),
            name: "test".into(),
            host: String::new(),
            variables: vec![crate::vars::EnvironmentVariable {
                id: uuid::Uuid::new_v4(),
                key: "q".into(),
                value: "ok".into(),
                enabled: true,
                secret: false,
                has_value: false,
                clear_secret: false,
            }],
        };
        let before = crate::vars::resolve_request(&source, Some(&env)).unwrap();
        let after = crate::vars::resolve_request(&imported.requests[0].spec, Some(&env)).unwrap();
        assert_eq!(
            crate::http::build_url(&before).unwrap(),
            crate::http::build_url(&after).unwrap()
        );
        assert!(!imported.requests[0].spec.params.last().unwrap().enabled);
    }
    #[test]
    fn all_bodies_and_auth_are_portable_without_private_lifecycle_metadata() {
        for body in [
            json!({"type":"json","content":"{}"}),
            json!({"type":"text","content":"<x/>","contentType":"application/custom+xml"}),
            json!({"type":"formData","rows":[{"key":"file","value":"fixture.txt","kind":"file","enabled":false,"description":"file"}]}),
            json!({"type":"binary","path":"fixture.bin"}),
            json!({"type":"graphQl","query":"query Q { id }","variables":"{}","operationName":"Q"}),
        ] {
            let source: RequestSpec = serde_json::from_value(json!({"method":"POST","url":"https://test.local/{id}","pathParams":[{"key":"id","value":"42","enabled":true}],"body":body, "auth":{"type":"oauth2","accessToken":"fixture","headerPrefix":"Bearer","expiresAt":100}})).unwrap();
            let exported = request(&source);
            assert!(!exported.to_string().contains("expiresAt"));
            let imported =
                super::super::import_json(&json!({"item":[{"request":exported}]}).to_string())
                    .unwrap();
            assert_eq!(
                serde_json::to_value(&source.body).unwrap(),
                serde_json::to_value(&imported.requests[0].spec.body).unwrap()
            );
            assert_eq!(
                crate::http::build_url(&source).unwrap(),
                crate::http::build_url(&imported.requests[0].spec).unwrap()
            );
        }
    }
}
