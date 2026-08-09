use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CookieJar {
    pub cookies: Vec<Cookie>,
}

impl CookieJar {
    pub fn set_cookie_header(&self, url: &str) -> String {
        let Ok(parsed) = Url::parse(url) else {
            return String::new();
        };
        self.cookies
            .iter()
            .filter(|c| {
                (c.domain.is_empty()
                    || parsed
                        .host_str()
                        .is_some_and(|h| h == c.domain || h.ends_with(&format!(".{}", c.domain))))
                    && (!c.secure || parsed.scheme() == "https")
                    && parsed.path().starts_with(&c.path)
            })
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ")
    }
    pub fn absorb(&mut self, url: &str, header: &str) {
        let Ok(parsed) = Url::parse(url) else { return };
        let mut parts = header.split(';');
        let Some(pair) = parts.next() else { return };
        let Some((name, value)) = pair.trim().split_once('=') else {
            return;
        };
        let mut cookie = Cookie {
            name: name.trim().into(),
            value: value.trim().into(),
            domain: parsed.host_str().unwrap_or("").into(),
            path: "/".into(),
            secure: false,
        };
        for attr in parts {
            let a = attr.trim();
            if a.eq_ignore_ascii_case("secure") {
                cookie.secure = true;
            } else if let Some(v) = a.strip_prefix("Path=").or_else(|| a.strip_prefix("path=")) {
                cookie.path = if v.starts_with('/') {
                    v.into()
                } else {
                    format!("/{v}")
                };
            } else if let Some(v) = a
                .strip_prefix("Domain=")
                .or_else(|| a.strip_prefix("domain="))
            {
                cookie.domain = v.trim_start_matches('.').into();
            }
        }
        self.cookies.retain(|c| {
            !(c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path)
        });
        self.cookies.push(cookie);
    }
}
