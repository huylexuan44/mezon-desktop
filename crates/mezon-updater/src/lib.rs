use sha2::{Digest, Sha512};
use std::sync::OnceLock;
use std::time::Duration;

pub const UPDATE_URL: &str = "https://cdn.mezon.ai/release/";

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

const ALLOWED_DOWNLOAD_HOSTS: &[&str] = &["mezon.ai", "cdn.mezon.ai"];

pub struct UpdateManifest {
    pub version: String,
    pub sha512: String,
    pub path: String,
}

pub fn validate_update_url(url: &str) -> anyhow::Result<()> {
    let parsed = url::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err(anyhow::anyhow!("rejected update URL: scheme must be https"));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("rejected update URL: no host"))?;
    if !ALLOWED_DOWNLOAD_HOSTS.contains(&host) {
        return Err(anyhow::anyhow!(
            "rejected update URL: host not in allowlist"
        ));
    }
    Ok(())
}

pub fn verify_file_checksum(file_bytes: &[u8], expected_sha512_b64: &str) -> anyhow::Result<()> {
    let mut hasher = Sha512::new();
    hasher.update(file_bytes);
    let digest = hasher.finalize();
    let actual_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        digest.as_slice(),
    );
    if actual_b64 != expected_sha512_b64 {
        return Err(anyhow::anyhow!(
            "checksum mismatch: expected {expected_sha512_b64}, got {actual_b64}"
        ));
    }
    Ok(())
}

fn manifest_filename() -> &'static str {
    if cfg!(target_os = "macos") {
        "latest-mac.yml"
    } else if cfg!(target_os = "windows") {
        "latest.yml"
    } else {
        "latest-linux.yml"
    }
}

fn parse_field<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(prefix.as_str()) {
            let v = rest.trim().trim_matches('\'').trim_matches('"');
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn parse_version_from_manifest(body: &str) -> anyhow::Result<semver::Version> {
    let v = parse_field(body, "version")
        .ok_or_else(|| anyhow::anyhow!("version field not found in update manifest"))?;
    semver::Version::parse(v).map_err(|e| anyhow::anyhow!("invalid semver in manifest: {e}"))
}

fn parse_manifest(body: &str) -> anyhow::Result<UpdateManifest> {
    let version = parse_version_from_manifest(body)?.to_string();
    let sha512 = parse_field(body, "sha512")
        .ok_or_else(|| anyhow::anyhow!("sha512 field not found in update manifest"))?
        .to_string();
    let path = parse_field(body, "path")
        .ok_or_else(|| anyhow::anyhow!("path field not found in update manifest"))?
        .to_string();
    Ok(UpdateManifest {
        version,
        sha512,
        path,
    })
}

pub async fn check_for_updates(current_version: &str) -> anyhow::Result<Option<String>> {
    match check_for_updates_with_manifest(current_version).await? {
        Some(m) => Ok(Some(m.version)),
        None => Ok(None),
    }
}

pub async fn check_for_updates_with_manifest(
    current_version: &str,
) -> anyhow::Result<Option<UpdateManifest>> {
    let current = semver::Version::parse(current_version)
        .map_err(|e| anyhow::anyhow!("invalid current version '{current_version}': {e}"))?;

    let manifest_url = format!("{}{}", UPDATE_URL, manifest_filename());
    validate_update_url(&manifest_url)?;

    tracing::debug!("fetching update manifest from {}", manifest_filename());

    let response = http_client()
        .get(&manifest_url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("update manifest fetch failed: {e}"))?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "update manifest returned HTTP {}",
            response.status()
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("failed to read update manifest body: {e}"))?;

    let manifest = parse_manifest(&body)?;
    let latest =
        semver::Version::parse(&manifest.version).expect("parse_manifest already validated");

    if latest > current {
        tracing::info!("update available: {} -> {}", current, latest);
        Ok(Some(manifest))
    } else {
        tracing::debug!("already up to date ({})", current);
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_mezon_ai_https() {
        assert!(validate_update_url("https://mezon.ai/download").is_ok());
    }

    #[test]
    fn validate_accepts_cdn_mezon_ai_https() {
        assert!(validate_update_url("https://cdn.mezon.ai/release/1.0.0/mezon.dmg").is_ok());
    }

    #[test]
    fn validate_rejects_http_scheme() {
        assert!(validate_update_url("http://mezon.ai/download").is_err());
    }

    #[test]
    fn validate_rejects_unknown_host() {
        assert!(validate_update_url("https://evil.com/mezon.dmg").is_err());
    }

    #[test]
    fn validate_rejects_subdomain_bypass() {
        assert!(validate_update_url("https://mezon.ai.evil.com/download").is_err());
    }

    #[test]
    fn validate_rejects_file_scheme() {
        assert!(validate_update_url("file:///tmp/malware").is_err());
    }

    #[test]
    fn validate_rejects_no_host() {
        assert!(validate_update_url("https:///no-host").is_err());
    }

    #[test]
    fn validate_rejects_malformed_url() {
        assert!(validate_update_url("not a url").is_err());
    }

    #[test]
    fn parse_version_bare() {
        let manifest = "version: 1.2.3\npath: mezon.dmg\nsha512: abc=\n";
        let v = parse_version_from_manifest(manifest).unwrap();
        assert_eq!(v, semver::Version::new(1, 2, 3));
    }

    #[test]
    fn parse_version_quoted_single() {
        let manifest = "version: '1.4.0'\npath: mezon.dmg\nsha512: abc=\n";
        let v = parse_version_from_manifest(manifest).unwrap();
        assert_eq!(v, semver::Version::new(1, 4, 0));
    }

    #[test]
    fn parse_version_quoted_double() {
        let manifest = "version: \"2.0.1\"\npath: mezon.dmg\nsha512: abc=\n";
        let v = parse_version_from_manifest(manifest).unwrap();
        assert_eq!(v, semver::Version::new(2, 0, 1));
    }

    #[test]
    fn parse_version_missing_returns_err() {
        let manifest = "files:\n  - url: mezon.dmg\n";
        assert!(parse_version_from_manifest(manifest).is_err());
    }

    #[test]
    fn parse_version_invalid_semver_returns_err() {
        let manifest = "version: not-a-version\n";
        assert!(parse_version_from_manifest(manifest).is_err());
    }

    #[test]
    fn manifest_filename_is_nonempty() {
        assert!(!manifest_filename().is_empty());
    }

    #[test]
    fn verify_file_checksum_accepts_correct_hash() {
        let data = b"hello mezon update";
        let mut hasher = Sha512::new();
        hasher.update(data);
        let digest = hasher.finalize();
        let expected = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            digest.as_slice(),
        );
        assert!(verify_file_checksum(data, &expected).is_ok());
    }

    #[test]
    fn verify_file_checksum_rejects_wrong_hash() {
        let data = b"hello mezon update";
        assert!(verify_file_checksum(data, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").is_err());
    }

    #[test]
    fn verify_file_checksum_rejects_empty_expected() {
        assert!(verify_file_checksum(b"data", "").is_err());
    }

    #[test]
    fn parse_manifest_extracts_all_fields() {
        let body = "version: 1.5.0\npath: mezon-1.5.0-mac.dmg\nsha512: abc123=\nreleaseDate: '2024-01-01'\n";
        let m = parse_manifest(body).unwrap();
        assert_eq!(m.version, "1.5.0");
        assert_eq!(m.path, "mezon-1.5.0-mac.dmg");
        assert_eq!(m.sha512, "abc123=");
    }

    #[test]
    fn parse_manifest_missing_sha512_returns_err() {
        let body = "version: 1.5.0\npath: mezon.dmg\n";
        assert!(parse_manifest(body).is_err());
    }

    #[test]
    fn parse_manifest_missing_path_returns_err() {
        let body = "version: 1.5.0\nsha512: abc=\n";
        assert!(parse_manifest(body).is_err());
    }
}
