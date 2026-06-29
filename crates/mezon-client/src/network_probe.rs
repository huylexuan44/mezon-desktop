use std::time::Duration;

use http_client::{AsyncBody, HttpClient, http};

use crate::transport_runtime;

pub const RECONNECT_NETWORK_PROBE_TIMEOUT: Duration = Duration::from_millis(4000);

const FALLBACK_PROBE_ORIGIN: &str = "https://mezon.ai";
const PROBE_PATH: &str = "/assets/favicon.ico";

pub fn favicon_probe_url(origin: &str) -> String {
    let trimmed = origin.trim_end_matches('/');
    let base = if trimmed.is_empty() {
        FALLBACK_PROBE_ORIGIN
    } else {
        trimmed
    };
    format!("{base}{PROBE_PATH}")
}

pub async fn probe_network_reachability(probe_url: &str, timeout: Duration) -> bool {
    let url = with_cache_buster(probe_url);

    let probe = transport_runtime::handle().spawn(async move {
        let request = match http::Request::builder()
            .method(http::Method::HEAD)
            .uri(&url)
            .body(AsyncBody::empty())
        {
            Ok(request) => request,
            Err(_) => return false,
        };
        matches!(
            tokio::time::timeout(timeout, transport_runtime::http_client().send(request)).await,
            Ok(Ok(_))
        )
    });

    probe.await.unwrap_or(false)
}

fn with_cache_buster(probe_url: &str) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let separator = if probe_url.contains('?') { '&' } else { '?' };
    format!("{probe_url}{separator}t={stamp}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favicon_probe_url_appends_path() {
        assert_eq!(
            favicon_probe_url("https://mezon.ai"),
            "https://mezon.ai/assets/favicon.ico"
        );
    }

    #[test]
    fn favicon_probe_url_trims_trailing_slash() {
        assert_eq!(
            favicon_probe_url("https://dev-mezon.nccsoft.vn/"),
            "https://dev-mezon.nccsoft.vn/assets/favicon.ico"
        );
    }

    #[test]
    fn favicon_probe_url_falls_back_on_empty_origin() {
        assert_eq!(favicon_probe_url(""), "https://mezon.ai/assets/favicon.ico");
    }

    #[test]
    fn cache_buster_uses_query_separator() {
        let url = with_cache_buster("https://mezon.ai/assets/favicon.ico");
        assert!(url.starts_with("https://mezon.ai/assets/favicon.ico?t="));
    }

    #[test]
    fn cache_buster_appends_to_existing_query() {
        let url = with_cache_buster("https://mezon.ai/assets/favicon.ico?v=1");
        assert!(url.starts_with("https://mezon.ai/assets/favicon.ico?v=1&t="));
    }
}
