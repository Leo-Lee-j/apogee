//! Authentication is applied to the finalized wire request, never to a URL template.
//! Signing failures deliberately contain field names, not credentials or JSON input.
use crate::{
    error::{AppError, ErrorKind},
    request::{AuthConfig, JwtAlgorithm, OAuth1SignatureMethod},
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use ring::hmac;

fn invalid(message: &str) -> AppError {
    AppError::new(ErrorKind::Parse, message)
}

/// RFC 5849 encoding: UTF-8, unreserved ASCII only, uppercase hex, never '+'.
fn percent(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            result.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}

fn object(
    value: &str,
    message: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, AppError> {
    match serde_json::from_str(value) {
        Ok(serde_json::Value::Object(value)) => Ok(value),
        _ => Err(invalid(message)),
    }
}

pub(crate) fn authorization_value(
    prefix: &str,
    token: &str,
) -> Result<reqwest::header::HeaderValue, AppError> {
    // A prefix is an HTTP authentication scheme, not arbitrary header text.
    if !prefix.is_empty()
        && !prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    {
        return Err(invalid("Auth Header Prefix 必须是合法的 HTTP 鉴权方案名"));
    }
    let value = if prefix.is_empty() {
        token.to_string()
    } else {
        format!("{prefix} {token}")
    };
    let mut value = reqwest::header::HeaderValue::from_str(&value)
        .map_err(|_| invalid("Auth 无法生成合法的 Authorization Header；请检查控制字符"))?;
    value.set_sensitive(true);
    Ok(value)
}

fn jwt(
    secret: &str,
    payload: &str,
    header: &str,
    algorithm: JwtAlgorithm,
) -> Result<String, AppError> {
    if secret.is_empty() {
        return Err(invalid("JWT Secret 不能为空"));
    }
    let payload = object(payload, "JWT Payload 必须是 JSON 对象")?;
    let mut header = object(header, "JWT Header 必须是 JSON 对象")?;
    let (name, algorithm) = match algorithm {
        JwtAlgorithm::HS256 => ("HS256", hmac::HMAC_SHA256),
        JwtAlgorithm::HS384 => ("HS384", hmac::HMAC_SHA384),
        JwtAlgorithm::HS512 => ("HS512", hmac::HMAC_SHA512),
    };
    if header.get("alg").is_some_and(|v| v.as_str() != Some(name)) {
        return Err(invalid("JWT Header alg 与所选签名算法不一致"));
    }
    if header.contains_key("crit") || header.contains_key("b64") {
        return Err(invalid("JWT 不支持 crit / b64 扩展 Header"));
    }
    header.insert("alg".into(), name.into());
    header.entry("typ").or_insert_with(|| "JWT".into());
    let encode = |value| {
        serde_json::to_vec(&value)
            .map(|v| URL_SAFE_NO_PAD.encode(v))
            .map_err(|_| invalid("JWT JSON 编码失败"))
    };
    let input = format!("{}.{}", encode(header)?, encode(payload)?);
    let signature = hmac::sign(
        &hmac::Key::new(algorithm, secret.as_bytes()),
        input.as_bytes(),
    );
    Ok(format!(
        "{input}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    ))
}

fn oauth_signature(
    request: &reqwest::Request,
    mut params: Vec<(String, String)>,
    consumer_secret: &str,
    token_secret: &str,
    algorithm: OAuth1SignatureMethod,
) -> Result<String, AppError> {
    params.extend(
        request
            .url()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned())),
    );
    let is_form = request
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        });
    if is_form {
        // Never buffer an upload stream to sign it. Form data participates only when the
        // final Content-Type says urlencoded and the finalized bytes are available.
        if let Some(body) = request.body() {
            let bytes = body
                .as_bytes()
                .ok_or_else(|| invalid("OAuth 1.0 不能签名流式 urlencoded Body"))?;
            params.extend(
                url::form_urlencoded::parse(bytes).map(|(k, v)| (k.into_owned(), v.into_owned())),
            );
        }
    }
    params.retain(|(k, _)| k != "oauth_signature");
    let mut encoded = params
        .into_iter()
        .map(|(k, v)| (percent(&k), percent(&v)))
        .collect::<Vec<_>>();
    encoded.sort(); // Sort encoded keys then encoded values; preserve duplicate pairs.
    let normalized = encoded
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let mut uri = request.url().clone();
    uri.set_query(None);
    uri.set_fragment(None);
    let _ = uri.set_username("");
    let _ = uri.set_password(None);
    let base = format!(
        "{}&{}&{}",
        request.method().as_str().to_ascii_uppercase(),
        percent(uri.as_str()),
        percent(&normalized)
    );
    let key = format!("{}&{}", percent(consumer_secret), percent(token_secret));
    let algorithm = match algorithm {
        OAuth1SignatureMethod::HmacSha1 => hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
        OAuth1SignatureMethod::HmacSha256 => hmac::HMAC_SHA256,
    };
    Ok(STANDARD
        .encode(hmac::sign(&hmac::Key::new(algorithm, key.as_bytes()), base.as_bytes()).as_ref()))
}

pub(crate) fn apply_signed_auth(
    request: &mut reqwest::Request,
    auth: &AuthConfig,
) -> Result<(), AppError> {
    // Explicit Authorization has exactly the same precedence for all auth schemes.
    if request
        .headers()
        .contains_key(reqwest::header::AUTHORIZATION)
    {
        return Ok(());
    }
    let value = match auth {
        AuthConfig::OAuth2 {
            access_token,
            header_prefix,
            ..
        } => {
            if access_token.is_empty() {
                return Err(invalid("OAuth 2.0 Access Token 不能为空"));
            }
            authorization_value(header_prefix, access_token)?
        }
        AuthConfig::Jwt {
            secret,
            payload,
            header,
            algorithm,
            header_prefix,
        } => authorization_value(header_prefix, &jwt(secret, payload, header, *algorithm)?)?,
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
        } => {
            if consumer_key.is_empty() || consumer_secret.is_empty() {
                return Err(invalid("OAuth 1.0 Consumer Key / Consumer Secret 不能为空"));
            }
            if request.headers().contains_key(reqwest::header::HOST) {
                return Err(invalid(
                    "OAuth 1.0 请禁用手写 Host Header，以确保签名 URI 与实际目标一致",
                ));
            }
            // Protocol parameters must have one source; do not silently double-sign input.
            let conflicting = |k: &str| k.starts_with("oauth_");
            if request.url().query_pairs().any(|(k, _)| conflicting(&k)) {
                return Err(invalid(
                    "OAuth 1.0 协议参数请仅在 Auth 面板配置，不要重复添加到 Query",
                ));
            }
            let is_form = request
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case("application/x-www-form-urlencoded")
                });
            if is_form
                && request
                    .body()
                    .and_then(|b| b.as_bytes())
                    .is_some_and(|b| url::form_urlencoded::parse(b).any(|(k, _)| conflicting(&k)))
            {
                return Err(invalid(
                    "OAuth 1.0 协议参数请仅在 Auth 面板配置，不要重复添加到 Body",
                ));
            }
            let nonce = if nonce.is_empty() {
                uuid::Uuid::new_v4().simple().to_string()
            } else {
                nonce.clone()
            };
            let timestamp = if timestamp.is_empty() {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| invalid("系统时间早于 Unix Epoch，无法生成 OAuth 1.0 Timestamp"))?
                    .as_secs()
                    .to_string()
            } else {
                if !timestamp.bytes().all(|b| b.is_ascii_digit())
                    || timestamp.parse::<u64>().is_err()
                {
                    return Err(invalid("OAuth 1.0 Timestamp 必须是 Unix 秒数（非负整数）"));
                }
                timestamp.clone()
            };
            let method = match signature_method {
                OAuth1SignatureMethod::HmacSha1 => "HMAC-SHA1",
                OAuth1SignatureMethod::HmacSha256 => "HMAC-SHA256",
            };
            let mut params = vec![
                ("oauth_consumer_key".into(), consumer_key.clone()),
                ("oauth_nonce".into(), nonce),
                ("oauth_signature_method".into(), method.into()),
                ("oauth_timestamp".into(), timestamp),
                ("oauth_version".into(), "1.0".into()),
            ];
            for (key, value) in [
                ("oauth_token", token),
                ("oauth_callback", callback),
                ("oauth_verifier", verifier),
            ] {
                if !value.is_empty() {
                    params.push((key.into(), value.clone()));
                }
            }
            let signature = oauth_signature(
                request,
                params.clone(),
                consumer_secret,
                token_secret,
                *signature_method,
            )?;
            params.push(("oauth_signature".into(), signature));
            if !realm.is_empty() {
                params.push(("realm".into(), realm.clone()));
            }
            params.sort();
            let header = params
                .into_iter()
                .map(|(k, v)| format!("{}=\"{}\"", percent(&k), percent(&v)))
                .collect::<Vec<_>>()
                .join(", ");
            authorization_value("OAuth", &header)?
        }
        _ => return Ok(()),
    };
    request
        .headers_mut()
        .insert(reqwest::header::AUTHORIZATION, value);
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn request(url: &str) -> reqwest::Request {
        reqwest::Request::new(reqwest::Method::GET, url.parse().unwrap())
    }
    pub(crate) fn oauth1() -> AuthConfig {
        AuthConfig::OAuth1 {
            consumer_key: "dpf43f3p2l4k3l03".into(),
            consumer_secret: "kd94hf93k423kf44".into(),
            token: "nnch734d00sl2jdk".into(),
            token_secret: "pfkkdhi9sl3r4s00".into(),
            signature_method: OAuth1SignatureMethod::HmacSha1,
            realm: String::new(),
            callback: String::new(),
            verifier: String::new(),
            nonce: "kllo9940pd9333jh".into(),
            timestamp: "1191242096".into(),
        }
    }
    fn header(req: &reqwest::Request) -> &str {
        req.headers()[reqwest::header::AUTHORIZATION]
            .to_str()
            .unwrap()
    }
    #[test]
    fn oauth_net_published_photo_vector_and_sensitive_header() {
        // https://oauth.net/core/1.0/ section 9.1.1 (includes oauth_version=1.0).
        let mut req = request("http://photos.example.net/photos?file=vacation.jpg&size=original");
        apply_signed_auth(&mut req, &oauth1()).unwrap();
        assert!(header(&req).contains("oauth_signature=\"tR3%2BTy81lMeYAr%2FFid0kMTYa%2FWM%3D\""));
        assert!(req.headers()[reqwest::header::AUTHORIZATION].is_sensitive());
        assert!(!header(&req).contains("kd94hf93k423kf44"));
        assert!(!header(&req).contains("pfkkdhi9sl3r4s00"));
    }
    #[test]
    fn rfc5849_post_vector_with_duplicates_empty_values_and_double_encoding() {
        // RFC 5849 section 3.4.1, corrected POST signature. The printed bYT5... is
        // the GET signature (erratum 2550); independently verified with Python HMAC.
        let mut req = request("http://example.com/request?b5=%3D%253D&a3=a&c%40=&a2=r%20b");
        *req.method_mut() = reqwest::Method::POST;
        req.headers_mut().insert(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded".parse().unwrap(),
        );
        *req.body_mut() = Some("c2&a3=2+q".into());
        let params = [
            ("oauth_consumer_key", "9djdj82h48djs9d2"),
            ("oauth_token", "kkk9d7dh3k39sjv7"),
            ("oauth_signature_method", "HMAC-SHA1"),
            ("oauth_timestamp", "137131201"),
            ("oauth_nonce", "7d8f3e4a"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        assert_eq!(
            oauth_signature(
                &req,
                params,
                "j49sk3j29djd",
                "dh893hdasih9",
                OAuth1SignatureMethod::HmacSha1
            )
            .unwrap(),
            "r6/TJjbCOr97/+UU0NsvSne7s5g="
        );
    }
    #[test]
    fn encoding_and_uri_normalization_are_rfc_compliant() {
        assert_eq!(percent("a +/=~中"), "a%20%2B%2F%3D~%E4%B8%AD");
        let mut a = request("HTTP://EXAMPLE.COM:80?z=2&z=1&x=a+b#fragment");
        let mut b = request("http://example.com/?x=a%20b&z=1&z=2");
        apply_signed_auth(&mut a, &oauth1()).unwrap();
        apply_signed_auth(&mut b, &oauth1()).unwrap();
        assert_eq!(header(&a), header(&b));
        let mut c = request("http://example.com:8080/?x=a%20b&z=1&z=2");
        apply_signed_auth(&mut c, &oauth1()).unwrap();
        assert_ne!(header(&a), header(&c));
    }
    #[test]
    fn realm_is_not_signed_callback_and_verifier_are_and_nonce_is_fresh() {
        let mut auth = oauth1();
        let mut a = request("https://example.test/");
        apply_signed_auth(&mut a, &auth).unwrap();
        if let AuthConfig::OAuth1 { realm, .. } = &mut auth {
            *realm = "test realm".into();
        }
        let mut b = request("https://example.test/");
        apply_signed_auth(&mut b, &auth).unwrap();
        assert!(header(&b).starts_with(header(&a).trim_end_matches(' ')));
        if let AuthConfig::OAuth1 {
            callback,
            verifier,
            nonce,
            timestamp,
            token,
            token_secret,
            ..
        } = &mut auth
        {
            *callback = "https://callback.test/?a=b".into();
            *verifier = "verification code".into();
            nonce.clear();
            timestamp.clear();
            token.clear();
            token_secret.clear();
        }
        let mut c = request("https://example.test/");
        let mut d = request("https://example.test/");
        apply_signed_auth(&mut c, &auth).unwrap();
        apply_signed_auth(&mut d, &auth).unwrap();
        assert_ne!(header(&c), header(&d));
        assert!(header(&c).contains("oauth_callback=\"https%3A%2F%2Fcallback.test%2F%3Fa%3Db\""));
        assert!(header(&c).contains("oauth_verifier=\"verification%20code\""));
        assert!(!header(&c).contains("oauth_token="));
    }
    #[test]
    fn body_participates_only_when_final_content_type_is_form() {
        let mut a = request("https://example.test/");
        *a.body_mut() = Some("a=b".into());
        let mut b = request("https://example.test/");
        b.headers_mut().insert(
            reqwest::header::CONTENT_TYPE,
            "application/json".parse().unwrap(),
        );
        *b.body_mut() = Some("a=c".into());
        apply_signed_auth(&mut a, &oauth1()).unwrap();
        apply_signed_auth(&mut b, &oauth1()).unwrap();
        assert_eq!(header(&a), header(&b));
        let mut c = request("https://example.test/");
        c.headers_mut().insert(
            reqwest::header::CONTENT_TYPE,
            "Application/X-Www-Form-Urlencoded; charset=UTF-8"
                .parse()
                .unwrap(),
        );
        *c.body_mut() = Some("a=b".into());
        apply_signed_auth(&mut c, &oauth1()).unwrap();
        assert_ne!(header(&a), header(&c));
    }
    #[test]
    fn reject_duplicate_protocol_parameters_invalid_timestamp_and_host() {
        for url in [
            "https://example.test/?oauth_nonce=secret",
            "https://example.test/?oauth_signature=secret",
        ] {
            let error = apply_signed_auth(&mut request(url), &oauth1()).unwrap_err();
            assert!(!serde_json::to_string(&error).unwrap().contains("secret"));
        }
        let mut req = request("https://example.test/");
        req.headers_mut()
            .insert(reqwest::header::HOST, "example.test".parse().unwrap());
        assert!(apply_signed_auth(&mut req, &oauth1()).is_err());
        let mut auth = oauth1();
        if let AuthConfig::OAuth1 { timestamp, .. } = &mut auth {
            *timestamp = "secret-timestamp".into();
        }
        assert!(!serde_json::to_string(
            &apply_signed_auth(&mut request("https://example.test/"), &auth).unwrap_err()
        )
        .unwrap()
        .contains("secret-timestamp"));
    }
    #[test]
    fn sha256_oauth_signature_is_not_sha1_and_uses_correct_method_name() {
        let mut auth = oauth1();
        if let AuthConfig::OAuth1 {
            signature_method, ..
        } = &mut auth
        {
            *signature_method = OAuth1SignatureMethod::HmacSha256;
        }
        let mut req = request("http://photos.example.net/photos?file=vacation.jpg&size=original");
        apply_signed_auth(&mut req, &auth).unwrap();
        assert!(header(&req).contains("oauth_signature_method=\"HMAC-SHA256\""));
        assert!(header(&req)
            .contains("oauth_signature=\"WVPzl1j6ZsnkIjWr7e3OZ3jkenL57KwaLFhYsroX1hg%3D\""));
    }
    #[test]
    fn jwt_fixed_vector_and_all_hmac_algorithms() {
        // Canonical JSON vector verified independently using Python hashlib / HMAC.
        assert_eq!(jwt("your-256-bit-secret",r#"{"sub":"1234567890","name":"John Doe","iat":1516239022}"#,"{}",JwtAlgorithm::HS256).unwrap(),"eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpYXQiOjE1MTYyMzkwMjIsIm5hbWUiOiJKb2huIERvZSIsInN1YiI6IjEyMzQ1Njc4OTAifQ.fdOPQ05ZfRhkST2-rIWgUpbqUsVhkkNVNcuG7Ki0s-8");
        for (algorithm, len) in [
            (JwtAlgorithm::HS256, 32),
            (JwtAlgorithm::HS384, 48),
            (JwtAlgorithm::HS512, 64),
        ] {
            let value = jwt(
                "secret",
                r#"{"sub":"user"}"#,
                r#"{"kid":"key-1"}"#,
                algorithm,
            )
            .unwrap();
            let parts = value.split('.').collect::<Vec<_>>();
            assert_eq!(parts.len(), 3);
            assert_eq!(URL_SAFE_NO_PAD.decode(parts[2]).unwrap().len(), len);
            assert!(String::from_utf8(URL_SAFE_NO_PAD.decode(parts[0]).unwrap())
                .unwrap()
                .contains("key-1"));
        }
    }
    #[test]
    fn jwt_invalid_json_alg_confusion_and_extensions_fail_without_echoing_input() {
        for (payload, header) in [
            ("secret-invalid", "{}"),
            ("[]", "{}"),
            ("{}", r#"{"alg":"none","secret":"private"}"#),
            ("{}", r#"{"crit":["x"]}"#),
            ("{}", r#"{"b64":false}"#),
        ] {
            let error = jwt("private", payload, header, JwtAlgorithm::HS256).unwrap_err();
            let text = serde_json::to_string(&error).unwrap();
            assert!(!text.contains("secret-invalid"));
            assert!(!text.contains("private"));
        }
    }
    #[test]
    fn oauth2_prefix_is_validated_and_explicit_authorization_wins() {
        for prefix in ["Bearer", "Token", ""] {
            let auth = AuthConfig::OAuth2 {
                access_token: "token".into(),
                header_prefix: prefix.into(),
                token_request: None,
                obtained_at: None,
                expires_at: None,
                auto_refresh: false,
                token_context: None,
            };
            let mut req = request("https://example.test/");
            apply_signed_auth(&mut req, &auth).unwrap();
            assert_eq!(
                header(&req),
                if prefix.is_empty() {
                    "token".into()
                } else {
                    format!("{prefix} token")
                }
            );
        }
        for (prefix, token) in [("Bearer bad", "secret"), ("Bearer", "secret\r\nX-Evil: y")] {
            let auth = AuthConfig::OAuth2 {
                access_token: token.into(),
                header_prefix: prefix.into(),
                token_request: None,
                obtained_at: None,
                expires_at: None,
                auto_refresh: false,
                token_context: None,
            };
            let mut req = request("https://example.test/");
            assert!(
                !serde_json::to_string(&apply_signed_auth(&mut req, &auth).unwrap_err())
                    .unwrap()
                    .contains("secret")
            );
            req.headers_mut().insert(
                reqwest::header::AUTHORIZATION,
                "Manual override".parse().unwrap(),
            );
            apply_signed_auth(&mut req, &auth).unwrap();
            assert_eq!(header(&req), "Manual override");
        }
    }
}
