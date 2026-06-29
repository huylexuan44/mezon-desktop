use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use futures::AsyncReadExt as _;
use futures::future::BoxFuture;
use http_client::{AsyncBody, HttpClient, Method, Request, Response, Url, http};
use reqwest_client::ReqwestClient;

use crate::transport_runtime::handle;

const DEFAULT_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
const EVICT_LOW_WATER_NUM: u64 = 9;
const EVICT_LOW_WATER_DEN: u64 = 10;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct DiskImageCacheClient {
    inner: ReqwestClient,
    dir: Option<PathBuf>,
    budget_bytes: u64,
    evicting: Arc<AtomicBool>,
}

impl DiskImageCacheClient {
    pub fn new(inner: ReqwestClient) -> Self {
        let dir = dirs::cache_dir().map(|base| base.join("mezon").join("avatars"));
        if let Some(dir) = dir.as_ref() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self {
            inner,
            dir,
            budget_bytes: DEFAULT_BUDGET_BYTES,
            evicting: Arc::new(AtomicBool::new(false)),
        }
    }
}

fn is_avatar_imgproxy_url(uri: &str) -> bool {
    uri.contains("rs:fill:100:100:") || uri.contains("rs:fit:300:300:")
}

fn cache_key(url: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}.webp")
}

fn ok_response(bytes: Vec<u8>) -> Response<AsyncBody> {
    Response::new(AsyncBody::from(bytes))
}

async fn read_cache(path: PathBuf) -> Option<Vec<u8>> {
    let bytes = handle()
        .spawn(async move { tokio::fs::read(&path).await.ok() })
        .await
        .ok()
        .flatten()?;
    (!bytes.is_empty()).then_some(bytes)
}

fn store_cache(
    dir: PathBuf,
    path: PathBuf,
    bytes: Vec<u8>,
    budget: u64,
    evicting: Arc<AtomicBool>,
) {
    handle().spawn(async move {
        if write_atomic(&path, &bytes).await.is_err() {
            return;
        }
        if evicting.swap(true, Ordering::SeqCst) {
            return;
        }
        enforce_budget(&dir, budget).await;
        evicting.store(false, Ordering::SeqCst);
    });
}

async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let token = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("{token}.tmp"));
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}

async fn enforce_budget(dir: &Path, budget: u64) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    let mut files: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
    let mut total: u64 = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("webp") {
            continue;
        }
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        total = total.saturating_add(meta.len());
        files.push((path, modified, meta.len()));
    }
    if total <= budget {
        return;
    }
    files.sort_by_key(|(_, modified, _)| *modified);
    let low_water = budget / EVICT_LOW_WATER_DEN * EVICT_LOW_WATER_NUM;
    for (path, _, len) in files {
        if total <= low_water {
            break;
        }
        if tokio::fs::remove_file(&path).await.is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

impl HttpClient for DiskImageCacheClient {
    fn user_agent(&self) -> Option<&http::HeaderValue> {
        self.inner.user_agent()
    }

    fn proxy(&self) -> Option<&Url> {
        self.inner.proxy()
    }

    fn send(
        &self,
        req: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let Some(dir) = self.dir.clone() else {
            return self.inner.send(req);
        };
        let uri = req.uri().to_string();
        if *req.method() != Method::GET || !is_avatar_imgproxy_url(&uri) {
            return self.inner.send(req);
        }
        let path = dir.join(cache_key(&uri));
        let budget = self.budget_bytes;
        let evicting = self.evicting.clone();
        let network = self.inner.send(req);
        Box::pin(async move {
            if let Some(bytes) = read_cache(path.clone()).await {
                return Ok(ok_response(bytes));
            }
            let mut response = network.await?;
            if !response.status().is_success() {
                return Ok(response);
            }
            let mut body = Vec::new();
            response.body_mut().read_to_end(&mut body).await?;
            if !body.is_empty() && body.len() as u64 <= MAX_ENTRY_BYTES {
                store_cache(dir, path, body.clone(), budget, evicting);
            }
            let (parts, _) = response.into_parts();
            Ok(Response::from_parts(parts, AsyncBody::from(body)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AVATAR_URL: &str = "https://dev-imgproxy.nccsoft.vn/k/rs:fill:100:100:1/mb:2097152/plain/https://cdn.mezon.ai/a.png@webp";
    const PROFILE_URL: &str = "https://dev-imgproxy.nccsoft.vn/k/rs:fit:300:300:1/mb:2097152/plain/https://cdn.mezon.ai/a.png@webp";

    #[test]
    fn cache_key_is_deterministic_and_distinct() {
        let other = AVATAR_URL.replace("/a.png@", "/b.png@");
        assert_eq!(cache_key(AVATAR_URL), cache_key(AVATAR_URL));
        assert_ne!(cache_key(AVATAR_URL), cache_key(&other));
        assert!(cache_key(AVATAR_URL).ends_with(".webp"));
    }

    #[test]
    fn only_avatar_and_profile_imgproxy_urls_are_cached() {
        assert!(is_avatar_imgproxy_url(AVATAR_URL));
        assert!(is_avatar_imgproxy_url(PROFILE_URL));
        assert!(!is_avatar_imgproxy_url(
            &AVATAR_URL.replace("rs:fill:100:100:", "rs:fill:800:600:")
        ));
        assert!(!is_avatar_imgproxy_url("https://api.mezon.ai/v2/account"));
    }

    #[tokio::test]
    async fn enforce_budget_keeps_cache_under_budget() {
        let dir = std::env::temp_dir().join(format!(
            "mezon-img-cache-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        for i in 0..5u8 {
            let path = dir.join(format!("{i:016x}.webp"));
            tokio::fs::write(&path, vec![i; 100]).await.unwrap();
        }

        enforce_budget(&dir, 250).await;

        let mut remaining: u64 = 0;
        let mut entries = tokio::fs::read_dir(&dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            if entry.path().extension().and_then(|ext| ext.to_str()) == Some("webp") {
                remaining += entry.metadata().await.unwrap().len();
            }
        }
        assert!(
            remaining <= 250,
            "cache not under budget: {remaining} bytes"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
