//! Official GitHub release checks and bounded, checksum-verified downloads.
//! SHA-256 establishes integrity against the official Release, not an independent signature.
use anyhow::{bail, ensure, Context, Result};
use futures_util::{Stream, StreamExt};
use ring::digest::{Context as Digest, SHA256};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;
pub use tokio_util::sync::CancellationToken;
use url::Url;

pub const REPOSITORY: &str = "Leo-Lee-j/apogee";
pub const MAX_PACKAGE_SIZE: u64 = 512 * 1024 * 1024;
const MAX_METADATA: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub notes: String,
    pub published_at: Option<String>,
    pub prerelease: bool,
    pub filename: String,
    pub size: u64,
    pub sha256: String,
}
impl Release {
    pub fn page_url(&self) -> String {
        format!("https://github.com/{REPOSITORY}/releases/tag/{}", self.tag)
    }
    fn asset_url(&self, filename: &str) -> String {
        format!(
            "https://github.com/{REPOSITORY}/releases/download/{}/{filename}",
            self.tag
        )
    }
    pub fn validate(&self) -> Result<()> {
        let version = Version::parse(&self.version).context("更新版本无效")?;
        ensure!(
            version.build.is_empty() && self.tag == format!("v{version}"),
            "更新标签无效"
        );
        ensure!(
            self.filename == format!("Apogee_{version}_x64-setup.exe"),
            "安装包名称不匹配"
        );
        ensure!(
            self.size > 0 && self.size <= MAX_PACKAGE_SIZE,
            "安装包大小无效"
        );
        ensure!(
            self.sha256.len() == 64 && self.sha256.bytes().all(|c| c.is_ascii_hexdigit()),
            "安装包 SHA-256 无效"
        );
        Ok(())
    }
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    body: Option<String>,
    published_at: Option<String>,
    assets: Vec<GithubAsset>,
}
#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub fn is_newer(release: &Release, current: &str) -> Result<bool> {
    Ok(Version::parse(&release.version).context("更新版本无效")?
        > Version::parse(current).context("当前应用版本无效")?)
}

/// Exact hosts, TLS and manual redirects; no request credentials or user proxy/TLS overrides.
fn allowed_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("更新地址无效")?;
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && url.fragment().is_none(),
        "更新地址必须使用 HTTPS"
    );
    ensure!(
        matches!(
            url.host_str(),
            Some(
                "api.github.com"
                    | "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        ),
        "更新下载来源不受信任"
    );
    Ok(url)
}
fn client() -> Result<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(30 * 60))
        .user_agent(concat!("Apogee/", env!("CARGO_PKG_VERSION")))
        .gzip(false)
        .brotli(false)
        .deflate(false)
        .build()
        .context("无法初始化更新连接")
}
async fn get(client: &reqwest::Client, address: &str) -> Result<reqwest::Response> {
    let mut url = allowed_url(address)?;
    for _ in 0..=5 {
        let response = client
            .get(url.clone())
            .header(
                "Accept",
                if url.host_str() == Some("api.github.com") {
                    "application/vnd.github+json"
                } else {
                    "application/octet-stream"
                },
            )
            .header("Accept-Encoding", "identity")
            .send()
            .await
            .map_err(|error| {
                anyhow::anyhow!(if error.is_timeout() {
                    "更新连接超时，请重试"
                } else {
                    "无法连接 GitHub 更新服务，请检查网络或系统代理"
                })
            })?;
        if response.status().is_redirection() {
            let next = response
                .headers()
                .get("location")
                .context("更新重定向缺少地址")?
                .to_str()
                .context("更新重定向地址无效")?;
            url = allowed_url(url.join(next).context("更新重定向地址无效")?.as_str())?;
        } else {
            return Ok(response);
        }
    }
    bail!("更新重定向次数过多")
}
fn require_success(response: &reqwest::Response) -> Result<()> {
    match response.status().as_u16() {
        200 => Ok(()),
        403 | 429 => bail!("GitHub 更新服务暂时拒绝访问或达到速率限制，请稍后重试"),
        404 => bail!("更新仓库或发布资产不可访问，请确认仓库和 Release 已公开且未删除"),
        _ => bail!(
            "GitHub 更新服务返回 HTTP {}，请稍后重试",
            response.status().as_u16()
        ),
    }
}
async fn metadata(client: &reqwest::Client, address: &str) -> Result<Vec<u8>> {
    let mut response = get(client, address).await?;
    require_success(&response)?;
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_METADATA as u64),
        "更新元数据过大"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.context("更新元数据读取失败")? {
        ensure!(bytes.len() + chunk.len() <= MAX_METADATA, "更新元数据过大");
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Compare actual semver tags, not GitHub ordering. Unpublished git tags are not installable.
fn select_release(bytes: &[u8], include_prerelease: bool) -> Result<Option<Release>> {
    let releases: Vec<GithubRelease> =
        serde_json::from_slice(bytes).context("GitHub 发布列表格式无效")?;
    let mut candidates = releases
        .into_iter()
        .filter_map(|item| {
            if item.draft || (!include_prerelease && item.prerelease) {
                return None;
            }
            let version = Version::parse(item.tag_name.strip_prefix('v')?).ok()?;
            if !version.build.is_empty() || (!include_prerelease && !version.pre.is_empty()) {
                return None;
            }
            Some((version, item))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    let Some((version, item)) = candidates.into_iter().next() else {
        return Ok(None);
    };
    let filename = format!("Apogee_{version}_x64-setup.exe");
    let url = |name: &str| {
        format!(
            "https://github.com/{REPOSITORY}/releases/download/{}/{name}",
            item.tag_name
        )
    };
    for name in [&filename, &"SHA256SUMS.txt".to_string()] {
        let assets = item
            .assets
            .iter()
            .filter(|a| &a.name == name)
            .collect::<Vec<_>>();
        ensure!(
            assets.len() == 1,
            "最新标签缺少唯一的安装包或 SHA256SUMS.txt"
        );
        ensure!(
            assets[0].browser_download_url == url(name),
            "发布资产地址与官方标签不匹配"
        );
        if name == &filename {
            ensure!(
                assets[0].size > 0 && assets[0].size <= MAX_PACKAGE_SIZE,
                "安装包大小无效"
            );
        }
    }
    let size = item
        .assets
        .iter()
        .find(|a| a.name == filename)
        .unwrap()
        .size;
    Ok(Some(Release {
        tag: item.tag_name,
        version: version.to_string(),
        notes: item.body.unwrap_or_default().chars().take(16_000).collect(),
        published_at: item.published_at,
        prerelease: item.prerelease || !version.pre.is_empty(),
        filename,
        size,
        sha256: String::new(),
    }))
}
fn checksum(bytes: &[u8], filename: &str) -> Result<String> {
    let text = std::str::from_utf8(bytes)
        .context("SHA256SUMS.txt 编码无效")?
        .trim_start_matches('\u{feff}');
    let mut found = None;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        ensure!(
            line.len() >= 66
                && line.as_bytes()[..64].iter().all(u8::is_ascii_hexdigit)
                && line.as_bytes()[64] == b' ',
            "SHA256SUMS.txt 格式无效，请使用逐行校验和"
        );
        let name = line[65..]
            .strip_prefix('*')
            .unwrap_or(&line[65..])
            .trim_start();
        if name == filename {
            ensure!(found.is_none(), "安装包存在重复校验和");
            found = Some(line[..64].to_ascii_lowercase());
        }
    }
    found.context("SHA256SUMS.txt 缺少该安装包校验和")
}
pub async fn check(include_prerelease: bool) -> Result<Option<Release>> {
    tokio::time::timeout(Duration::from_secs(45), async {
        let client = client()?;
        let address = format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=100");
        let bytes = metadata(&client, &address).await?;
        let Some(mut release) = select_release(&bytes, include_prerelease)? else {
            return Ok(None);
        };
        let sums = metadata(&client, &release.asset_url("SHA256SUMS.txt")).await?;
        release.sha256 = checksum(&sums, &release.filename)?;
        release.validate()?;
        Ok(Some(release))
    })
    .await
    .context("检查更新超时，请稍后重试")?
}
fn hash_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Reject symlinks/reparse points before reading or publishing executable files.
fn regular_file(path: &Path) -> Result<std::fs::Metadata> {
    let meta = std::fs::symlink_metadata(path).context("安装包文件不可用")?;
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink(),
        "安装包不是普通文件"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            meta.file_attributes() & 0x400 == 0,
            "安装包不能是链接或重解析点"
        );
    }
    Ok(meta)
}
pub fn verify_package(path: &Path, release: &Release) -> Result<()> {
    use std::io::Read;
    release.validate()?;
    ensure!(
        regular_file(path)?.len() == release.size,
        "安装包大小校验失败"
    );
    let mut file = std::fs::File::open(path).context("无法读取安装包")?;
    let mut hash = Digest::new(&SHA256);
    let mut buffer = [0; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer).context("无法读取安装包")?;
        if count == 0 {
            break;
        }
        total += count as u64;
        ensure!(total <= release.size, "安装包大小校验失败");
        hash.update(&buffer[..count]);
    }
    ensure!(
        total == release.size
            && hash_hex(hash.finish().as_ref()) == release.sha256.to_ascii_lowercase(),
        "安装包 SHA-256 校验失败，请重新下载"
    );
    Ok(())
}
struct PartialFile(PathBuf);
impl Drop for PartialFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub async fn download(
    release: &Release,
    cache: &Path,
    cancel: &CancellationToken,
    progress: impl Fn(u64),
) -> Result<PathBuf> {
    release.validate()?;
    with_cancel(cancel, download_inner(release, cache, &progress)).await
}
async fn with_cancel<T>(
    cancel: &CancellationToken,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    // Cancellation also interrupts connect/TTFB, stalled streams and filesystem futures.
    tokio::select! {
        biased;
        _ = cancel.cancelled() => bail!("UPDATE_CANCELLED: 下载已取消"),
        result = future => result,
    }
}

async fn download_inner(
    release: &Release,
    cache: &Path,
    progress: &impl Fn(u64),
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(cache)
        .await
        .context("无法创建更新缓存目录")?;
    ensure!(
        !std::fs::symlink_metadata(cache)?.file_type().is_symlink(),
        "更新缓存目录不能是链接"
    );
    let path = cache.join(&release.filename);
    if path.exists() {
        regular_file(&path)?;
        let file = path.clone();
        let expected = release.clone();
        if tokio::task::spawn_blocking(move || verify_package(&file, &expected))
            .await?
            .is_ok()
        {
            progress(release.size);
            return Ok(path);
        }
    }
    let response = get(&client()?, &release.asset_url(&release.filename)).await?;
    require_success(&response)?;
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size == release.size),
        "下载响应大小与发布资产不匹配"
    );
    save_package(release, cache, progress, response.bytes_stream()).await
}
async fn save_package<S, T, E>(
    release: &Release,
    cache: &Path,
    progress: &impl Fn(u64),
    mut chunks: S,
) -> Result<PathBuf>
where
    S: Stream<Item = std::result::Result<T, E>> + Unpin,
    T: AsRef<[u8]>,
    E: Into<anyhow::Error>,
{
    let path = cache.join(&release.filename);
    let partial = PartialFile(cache.join(format!(
        "{}.{}.part",
        release.filename,
        uuid::Uuid::new_v4()
    )));
    let mut output = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial.0)
        .await
        .context("无法写入更新缓存，请检查磁盘空间")?;
    let mut hash = Digest::new(&SHA256);
    let mut downloaded = 0_u64;
    let mut notified = Instant::now();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk
            .map_err(Into::into)
            .context("安装包读取失败，请重试")?;
        let chunk = chunk.as_ref();
        downloaded += chunk.len() as u64;
        ensure!(downloaded <= release.size, "安装包超出发布声明的大小");
        output
            .write_all(chunk)
            .await
            .context("安装包写入失败，请检查磁盘空间")?;
        hash.update(chunk);
        if notified.elapsed() >= Duration::from_millis(100) {
            progress(downloaded);
            notified = Instant::now();
        }
    }
    ensure!(
        downloaded == release.size
            && hash_hex(hash.finish().as_ref()) == release.sha256.to_ascii_lowercase(),
        "安装包大小或 SHA-256 校验失败，请重试"
    );
    output.flush().await.context("安装包写入失败")?;
    output.sync_all().await.context("安装包写入失败")?;
    drop(output);
    // Only replace this exact, previously validated cache file, never a user-selected path.
    if path.exists() {
        regular_file(&path)?;
        tokio::fs::remove_file(&path)
            .await
            .context("无法替换旧安装包")?;
    }
    tokio::fs::rename(&partial.0, &path)
        .await
        .context("无法发布已校验的安装包")?;
    progress(downloaded);
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn github(tag: &str, draft: bool, preview: bool) -> serde_json::Value {
        let v = tag.trim_start_matches('v');
        let filename = format!("Apogee_{v}_x64-setup.exe");
        serde_json::json!({"tag_name":tag,"draft":draft,"prerelease":preview,"body":"release notes","published_at":null,
            "assets":[{"name":filename,"size":3,"browser_download_url":format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{filename}")},
            {"name":"SHA256SUMS.txt","size":80,"browser_download_url":format!("https://github.com/{REPOSITORY}/releases/download/{tag}/SHA256SUMS.txt")}]})
    }
    fn release() -> Release {
        Release {
            tag: "v1.2.3".into(),
            version: "1.2.3".into(),
            notes: String::new(),
            published_at: None,
            prerelease: false,
            filename: "Apogee_1.2.3_x64-setup.exe".into(),
            size: 3,
            sha256: hash_hex(ring::digest::digest(&SHA256, b"abc").as_ref()),
        }
    }
    #[test]
    fn versions_are_semver_ordered_and_channels_filter_drafts() {
        let bytes = serde_json::to_vec(&vec![
            github("v1.9.0", false, false),
            github("v1.10.0", false, false),
            github("v2.0.0-beta.1", false, true),
            github("v9.0.0", true, false),
        ])
        .unwrap();
        assert_eq!(
            select_release(&bytes, false).unwrap().unwrap().version,
            "1.10.0"
        );
        assert_eq!(
            select_release(&bytes, true).unwrap().unwrap().version,
            "2.0.0-beta.1"
        );
        assert!(is_newer(&release(), "1.2.3-beta.1").unwrap());
        assert!(!is_newer(&release(), "1.2.3").unwrap());
        assert!(!is_newer(&release(), "2.0.0").unwrap());
    }
    #[test]
    fn empty_and_non_version_tags_are_not_updates() {
        assert!(select_release(b"[]", false).unwrap().is_none());
        assert!(select_release(
            &serde_json::to_vec(&vec![github("nightly", false, false)]).unwrap(),
            true
        )
        .unwrap()
        .is_none());
    }
    #[test]
    fn latest_release_must_have_exact_official_assets() {
        let mut entry = github("v1.2.3", false, false);
        entry["assets"][0]["browser_download_url"] =
            serde_json::json!("https://evil.example/setup.exe");
        assert!(select_release(&serde_json::to_vec(&vec![entry]).unwrap(), false).is_err());
        let mut entry = github("v1.2.3", false, false);
        entry["assets"] = serde_json::json!([]);
        assert!(select_release(&serde_json::to_vec(&vec![entry]).unwrap(), false).is_err());
    }
    #[test]
    fn redirects_require_exact_github_hosts_and_tls() {
        for value in [
            "http://github.com/a",
            "https://github.com.evil.test/a",
            "https://user@github.com/a",
            "https://github.com:444/a",
            "https://localhost/a",
            "https://github.com/a#fragment",
        ] {
            assert!(allowed_url(value).is_err(), "{value}");
        }
        assert!(
            allowed_url("https://release-assets.githubusercontent.com/a?signature=abc").is_ok()
        );
    }
    #[test]
    fn checksums_accept_standard_formats_and_reject_duplicates_or_missing() {
        let r = release();
        for line in [
            format!("{} *{}\r\n", r.sha256, r.filename),
            format!("{}  {}\n", r.sha256.to_uppercase(), r.filename),
        ] {
            assert_eq!(checksum(line.as_bytes(), &r.filename).unwrap(), r.sha256);
        }
        let line = format!("{} *{}\n", r.sha256, r.filename);
        assert!(checksum(line.repeat(2).as_bytes(), &r.filename).is_err());
        assert!(checksum(b"garbage", &r.filename).is_err());
        assert!(checksum(line.as_bytes(), "missing.exe").is_err());
        let malformed = format!("{} *{}{} *other.exe", r.sha256, r.filename, r.sha256);
        assert!(checksum(malformed.as_bytes(), &r.filename).is_err());
    }
    #[test]
    fn packages_reject_truncation_same_size_corruption_and_unsafe_names() {
        let path =
            std::env::temp_dir().join(format!("apogee-update-test-{}", uuid::Uuid::new_v4()));
        let r = release();
        std::fs::write(&path, b"abc").unwrap();
        assert!(verify_package(&path, &r).is_ok());
        std::fs::write(&path, b"abd").unwrap();
        assert!(verify_package(&path, &r).is_err());
        std::fs::write(&path, b"ab").unwrap();
        assert!(verify_package(&path, &r).is_err());
        std::fs::remove_file(path).unwrap();
        let mut unsafe_release = r;
        unsafe_release.filename = "../setup.exe".into();
        assert!(unsafe_release.validate().is_err());
    }
    #[tokio::test]
    async fn cancellation_happens_before_network_or_disk_writes() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let path =
            std::env::temp_dir().join(format!("apogee-update-cancel-{}", uuid::Uuid::new_v4()));
        let error = download(&release(), &path, &cancel, |_| {})
            .await
            .unwrap_err();
        assert!(error.to_string().starts_with("UPDATE_CANCELLED"));
        assert!(!path.exists());
    }
    #[test]
    fn partial_files_are_removed_when_operation_drops() {
        let path =
            std::env::temp_dir().join(format!("apogee-update-partial-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"partial").unwrap();
        {
            let _guard = PartialFile(path.clone());
        }
        assert!(!path.exists());
    }
    fn test_cache() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("apogee-update-stream-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        path
    }
    #[tokio::test]
    async fn streamed_package_is_only_published_after_verification() {
        let cache = test_cache();
        let r = release();
        let chunks = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(b"a".to_vec()),
            Ok(b"bc".to_vec()),
        ]);
        let path = save_package(&r, &cache, &|_| {}, chunks).await.unwrap();
        verify_package(&path, &r).unwrap();
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(cache).unwrap();
    }
    #[tokio::test]
    async fn failed_streams_leave_no_installer_or_partial_file() {
        for bytes in [b"ab".to_vec(), b"abd".to_vec(), b"abcd".to_vec()] {
            let cache = test_cache();
            let chunks = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(bytes)]);
            assert!(save_package(&release(), &cache, &|_| {}, chunks)
                .await
                .is_err());
            assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
            std::fs::remove_dir(cache).unwrap();
        }
        let cache = test_cache();
        let chunks = futures_util::stream::iter(vec![
            Ok(b"a".to_vec()),
            Err(std::io::Error::other("interrupted")),
        ]);
        assert!(save_package(&release(), &cache, &|_| {}, chunks)
            .await
            .is_err());
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
        std::fs::remove_dir(cache).unwrap();
    }
    #[tokio::test]
    async fn stalled_stream_cancellation_removes_partial_file() {
        let cache = test_cache();
        let target = cache.clone();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            let chunks = futures_util::stream::iter(vec![Ok::<_, std::io::Error>(b"a".to_vec())])
                .chain(futures_util::stream::pending());
            with_cancel(&token, save_package(&release(), &target, &|_| {}, chunks)).await
        });
        for _ in 0..100 {
            if std::fs::read_dir(&cache).unwrap().count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
        cancel.cancel();
        assert!(task
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .starts_with("UPDATE_CANCELLED"));
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
        std::fs::remove_dir(cache).unwrap();
    }
}
