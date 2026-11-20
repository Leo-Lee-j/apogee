//! Bounded in-memory workspace sessions. Never persisted to workspace/history/logs.
use cookie_store::{Cookie, CookieDomain, CookieExpiration};
use reqwest::{cookie::CookieStore, header::HeaderValue};
use serde::Serialize;
use std::sync::Mutex;
use url::Url;
use uuid::Uuid;

const MAX_COOKIES: usize = 300;
const MAX_DOMAIN_COOKIES: usize = 50;
const MAX_COOKIE_BYTES: usize = 4096;
struct Entry {
    id: Uuid,
    cookie: Cookie<'static>,
}
#[derive(Default)]
pub struct CookieJar {
    entries: Mutex<Vec<Entry>>,
}

/// Explicit manager DTO, never part of a public request projection.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCookie {
    pub id: Uuid,
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub host_only: bool,
    pub secure: bool,
    pub http_only: bool,
    pub expires_at: Option<i64>,
    pub header: String,
}
fn domain<'a>(cookie: &'a Cookie<'_>) -> &'a str {
    match &cookie.domain {
        CookieDomain::HostOnly(value) | CookieDomain::Suffix(value) => value,
        _ => "",
    }
}
fn identity(a: &Cookie<'_>, b: &Cookie<'_>) -> bool {
    a.name() == b.name() && domain(a) == domain(b) && *a.path == *b.path
}
fn safe_cookie(cookie: &Cookie<'_>) -> bool {
    !cookie.name().is_empty()
        && cookie
            .name()
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == 0x60 || b"!#$%&'*+-.^_|~".contains(&c))
        && cookie.value().bytes().all(|c| {
            c == 0x21
                || (0x23..=0x2b).contains(&c)
                || (0x2d..=0x3a).contains(&c)
                || (0x3c..=0x5b).contains(&c)
                || (0x5d..=0x7e).contains(&c)
        })
}
impl CookieJar {
    pub fn put(&self, url: &Url, header: &str, replace: Option<Uuid>) -> Result<(), &'static str> {
        if !matches!(url.scheme(), "http" | "https")
            || header.len() > MAX_COOKIE_BYTES
            || header.contains(['\r', '\n'])
        {
            return Err("Cookie 来源或长度无效（每项最多 4096 字节）");
        }
        let mut cookie = Cookie::parse(header.to_string(), url)
            .map_err(|_| "Cookie 格式、域或来源不合法")?
            .into_owned();
        if !safe_cookie(&cookie) {
            return Err("Cookie 名或值不符合 HTTP 格式");
        }
        if cookie.secure().unwrap_or(false) && url.scheme() != "https" {
            return Err("Secure Cookie 只接受 HTTPS 来源");
        }
        if cookie.name().starts_with("__Secure-") && !cookie.secure().unwrap_or(false) {
            return Err("__Secure- Cookie 必须设置 Secure");
        }
        if cookie.name().starts_with("__Host-")
            && (!cookie.secure().unwrap_or(false)
                || cookie.domain().is_some()
                || cookie.path() != Some("/"))
        {
            return Err("__Host- Cookie 必须为 Secure、Path=/ 且不含 Domain");
        }
        if let CookieDomain::Suffix(value) = &cookie.domain {
            let value = value.to_ascii_lowercase();
            if psl::suffix_str(&value) == Some(value.as_str())
                || value.parse::<std::net::IpAddr>().is_ok()
            {
                if url.host_str() != Some(value.as_str()) {
                    return Err("Cookie Domain 不能是公共后缀或其他 IP");
                }
                cookie.domain = CookieDomain::HostOnly(value);
            }
        }
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|entry| !entry.cookie.is_expired());
        if replace.is_some_and(|id| !entries.iter().any(|entry| entry.id == id)) {
            return Err("Cookie 已失效，请刷新列表");
        }
        if url.scheme() != "https"
            && entries.iter().any(|entry| {
                identity(&entry.cookie, &cookie) && entry.cookie.secure().unwrap_or(false)
            })
        {
            return Err("HTTP 来源不能覆盖 Secure Cookie");
        }
        let existing = entries
            .iter()
            .position(|entry| identity(&entry.cookie, &cookie));
        let id = existing
            .map(|i| entries[i].id)
            .or(replace)
            .unwrap_or_else(Uuid::new_v4);
        if cookie.is_expired() {
            entries.retain(|entry| !identity(&entry.cookie, &cookie) && Some(entry.id) != replace);
            return Ok(());
        }
        if let Some(index) = existing {
            entries[index] = Entry { id, cookie };
            entries.retain(|entry| Some(entry.id) != replace || entry.id == id);
        } else {
            entries.retain(|entry| Some(entry.id) != replace);
            if entries
                .iter()
                .filter(|entry| domain(&entry.cookie) == domain(&cookie))
                .count()
                >= MAX_DOMAIN_COOKIES
            {
                if let Some(index) = entries
                    .iter()
                    .position(|entry| domain(&entry.cookie) == domain(&cookie))
                {
                    entries.remove(index);
                }
            }
            if entries.len() >= MAX_COOKIES {
                entries.remove(0);
            }
            entries.push(Entry { id, cookie });
        }
        Ok(())
    }
    pub fn list(&self) -> Vec<SessionCookie> {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|entry| !entry.cookie.is_expired());
        entries
            .iter()
            .map(|entry| {
                let cookie = &entry.cookie;
                SessionCookie {
                    id: entry.id,
                    name: cookie.name().into(),
                    value: cookie.value().into(),
                    domain: domain(cookie).into(),
                    path: (*cookie.path).into(),
                    host_only: matches!(cookie.domain, CookieDomain::HostOnly(_)),
                    secure: cookie.secure().unwrap_or(false),
                    http_only: cookie.http_only().unwrap_or(false),
                    expires_at: match cookie.expires {
                        CookieExpiration::SessionEnd => None,
                        CookieExpiration::AtUtc(at) => Some(at.unix_timestamp()),
                    },
                    header: {
                        let mut header = format!(
                            "{}={}; Path={}",
                            cookie.name(),
                            cookie.value(),
                            &*cookie.path
                        );
                        if let CookieDomain::Suffix(domain) = &cookie.domain {
                            header.push_str(&format!("; Domain={domain}"));
                        }
                        if cookie.secure().unwrap_or(false) {
                            header.push_str("; Secure");
                        }
                        if cookie.http_only().unwrap_or(false) {
                            header.push_str("; HttpOnly");
                        }
                        if let CookieExpiration::AtUtc(at) = cookie.expires {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs() as i64;
                            header.push_str(&format!(
                                "; Max-Age={}",
                                at.unix_timestamp().saturating_sub(now).max(0)
                            ));
                        }
                        header
                    },
                }
            })
            .collect()
    }
    pub fn delete(&self, id: Option<Uuid>) {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|entry| id.is_some_and(|id| entry.id != id));
    }
}
impl CookieStore for CookieJar {
    fn set_cookies(&self, headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        // Never log network cookies or parser errors.
        for header in headers {
            if let Ok(header) = header.to_str() {
                let _ = self.put(url, header, None);
            }
        }
    }
    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|entry| !entry.cookie.is_expired());
        let mut matching: Vec<_> = entries
            .iter()
            .filter(|entry| entry.cookie.matches(url))
            .collect();
        matching.sort_by_key(|entry| std::cmp::Reverse(entry.cookie.path.len()));
        let value = matching
            .iter()
            .map(|entry| format!("{}={}", entry.cookie.name(), entry.cookie.value()))
            .collect::<Vec<_>>()
            .join("; ");
        if value.is_empty() {
            None
        } else {
            HeaderValue::from_str(&value).ok()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn url(value: &str) -> Url {
        Url::parse(value).unwrap()
    }
    fn sent(jar: &CookieJar, value: &str) -> String {
        jar.cookies(&url(value))
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }
    #[test]
    fn domain_path_expiry_secure_and_prefix_rules() {
        let jar = CookieJar::default();
        let source = url("https://api.example.com/login/path");
        jar.put(&source, "host=one; HttpOnly", None).unwrap();
        jar.put(
            &source,
            "shared=two; Domain=example.com; Path=/; Secure",
            None,
        )
        .unwrap();
        assert_eq!(
            sent(&jar, "https://api.example.com/login/path"),
            "host=one; shared=two"
        );
        assert_eq!(
            sent(&jar, "https://sub.api.example.com/login/path"),
            "shared=two"
        );
        assert_eq!(
            sent(&jar, "https://api.example.com/login-other"),
            "shared=two"
        );
        assert_eq!(sent(&jar, "http://api.example.com/login/path"), "host=one");
        assert!(sent(&jar, "https://example.com.evil.test/login").is_empty());
        for header in [
            "bad=x; Domain=com",
            "bad=x; Domain=other.test",
            "__Host-a=x; Secure; Domain=example.com; Path=/",
        ] {
            assert!(jar.put(&source, header, None).is_err());
        }
        assert!(jar
            .put(&url("http://api.example.com/"), "x=y; Secure", None)
            .is_err());
        jar.put(
            &source,
            "shared=gone; Domain=example.com; Path=/; Secure; Max-Age=0",
            None,
        )
        .unwrap();
        assert_eq!(jar.list().len(), 1);
        let id = jar.list()[0].id;
        jar.put(&source, "host=edited; Path=/login; HttpOnly", Some(id))
            .unwrap();
        assert_eq!(jar.list()[0].value, "edited");
        jar.delete(Some(id));
        assert!(jar.list().is_empty());
    }
    #[test]
    fn bounded_memory_and_failed_edits() {
        let jar = CookieJar::default();
        for index in 0..400 {
            jar.put(&url(&format!("https://h{index}.example.com/")), "a=b", None)
                .unwrap();
        }
        assert_eq!(jar.list().len(), MAX_COOKIES);
        assert!(jar
            .put(
                &url("https://example.com/"),
                &format!("x={}", "x".repeat(MAX_COOKIE_BYTES)),
                Some(jar.list()[0].id)
            )
            .is_err());
        assert_eq!(jar.list().len(), MAX_COOKIES);
        jar.delete(None);
        for index in 0..100 {
            jar.put(&url("https://example.com/"), &format!("c{index}=v"), None)
                .unwrap();
        }
        assert_eq!(jar.list().len(), MAX_DOMAIN_COOKIES);
        assert!(CookieJar::default().list().is_empty());
    }
}
