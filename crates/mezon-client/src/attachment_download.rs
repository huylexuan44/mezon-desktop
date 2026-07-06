use std::path::{Path, PathBuf};

pub fn clean_download_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let cleaned = trimmed.replace("@webp", "");
    let parsed = url::Url::parse(&cleaned).ok()?;
    match parsed.scheme() {
        "http" | "https" => Some(cleaned),
        _ => None,
    }
}

pub fn resolve_download_filename(filename: &str, url: &str) -> String {
    let from_name = sanitize_filename(filename);
    if from_name != "download" || !filename.trim().is_empty() {
        let has_ext = Path::new(&from_name)
            .extension()
            .is_some_and(|e| !e.to_str().unwrap_or("").is_empty());
        if has_ext {
            return from_name;
        }
    }
    if let Some(segment) = url
        .split(['?', '#'])
        .next()
        .and_then(|path| path.rsplit('/').next())
        .filter(|s| !s.is_empty())
    {
        let from_url = sanitize_filename(segment);
        if from_url != "download" {
            return from_url;
        }
    }
    from_name
}

pub fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        "download".to_string()
    } else {
        cleaned
    }
}

pub async fn download_url_to_downloads(url: &str, filename: &str) -> anyhow::Result<PathBuf> {
    let url = clean_download_url(url).ok_or_else(|| anyhow::anyhow!("invalid download url"))?;
    let filename = resolve_download_filename(filename, &url);
    let (bytes, _) = crate::transport_runtime::fetch_bytes(&url).await?;
    write_bytes_to_downloads(&filename, &bytes).await
}

pub async fn write_bytes_to_downloads(filename: &str, bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| anyhow::anyhow!("no download directory available"))?;
    let filename = filename.to_string();
    let bytes = bytes.to_vec();
    crate::transport_runtime::handle()
        .spawn_blocking(move || write_bytes_to_downloads_sync(&dir, &filename, &bytes))
        .await
        .map_err(|e| anyhow::anyhow!("file write task failed: {e}"))?
}

fn write_bytes_to_downloads_sync(
    dir: &Path,
    filename: &str,
    bytes: &[u8],
) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir).ok();

    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = path.extension().and_then(|s| s.to_str());

    let mut candidate = dir.join(filename);
    let mut counter = 1u32;
    while candidate.exists() {
        let name = match ext {
            Some(ext) => format!("{stem} ({counter}).{ext}"),
            None => format!("{stem} ({counter})"),
        };
        candidate = dir.join(name);
        counter += 1;
        if counter > 9999 {
            break;
        }
    }

    std::fs::write(&candidate, bytes)?;
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_download_url_strips_webp_suffix() {
        let url = "https://cdn.example.com/a.png@webp";
        assert_eq!(
            clean_download_url(url).as_deref(),
            Some("https://cdn.example.com/a.png")
        );
    }

    #[test]
    fn clean_download_url_rejects_non_http() {
        assert!(clean_download_url("file:///tmp/x").is_none());
    }

    #[test]
    fn resolve_filename_falls_back_to_url_segment() {
        assert_eq!(
            resolve_download_filename("", "https://cdn.example.com/photo.jpg?token=1"),
            "photo.jpg"
        );
    }
}
