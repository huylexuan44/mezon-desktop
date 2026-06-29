use std::path::{Path, PathBuf};

pub const MAX_FILE_ATTACHMENTS: usize = 50;
pub const IMAGE_MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;
pub const MAX_FILE_SIZE: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PendingAttachment {
    pub path: PathBuf,
    pub filename: String,
    pub filetype: String,
    pub size: u64,
    pub is_image: bool,
}

pub fn mime_from_extension(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    };
    mime.to_string()
}

pub fn size_within_limit(size: u64, is_image: bool) -> bool {
    let limit = if is_image {
        IMAGE_MAX_FILE_SIZE
    } else {
        MAX_FILE_SIZE
    };
    size <= limit
}

pub fn build_pending(path: PathBuf) -> Option<PendingAttachment> {
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let filename = path.file_name()?.to_str()?.to_string();
    let filetype = mime_from_extension(&path);
    let is_image = filetype.starts_with("image/");
    Some(PendingAttachment {
        path,
        filename,
        filetype,
        size: meta.len(),
        is_image,
    })
}

pub fn acceptable(existing: usize, candidate: &PendingAttachment) -> bool {
    existing < MAX_FILE_ATTACHMENTS && size_within_limit(candidate.size, candidate.is_image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_maps_known_extensions() {
        assert_eq!(mime_from_extension(Path::new("a.PNG")), "image/png");
        assert_eq!(mime_from_extension(Path::new("a.jpeg")), "image/jpeg");
        assert_eq!(mime_from_extension(Path::new("clip.mp4")), "video/mp4");
        assert_eq!(mime_from_extension(Path::new("doc.pdf")), "application/pdf");
        assert_eq!(
            mime_from_extension(Path::new("noext")),
            "application/octet-stream"
        );
    }

    #[test]
    fn size_limits_differ_for_image_vs_file() {
        assert!(size_within_limit(IMAGE_MAX_FILE_SIZE, true));
        assert!(!size_within_limit(IMAGE_MAX_FILE_SIZE + 1, true));
        assert!(size_within_limit(IMAGE_MAX_FILE_SIZE + 1, false));
        assert!(size_within_limit(MAX_FILE_SIZE, false));
        assert!(!size_within_limit(MAX_FILE_SIZE + 1, false));
    }

    #[test]
    fn acceptable_enforces_count_and_size() {
        let img = PendingAttachment {
            path: PathBuf::from("a.png"),
            filename: "a.png".into(),
            filetype: "image/png".into(),
            size: 1024,
            is_image: true,
        };
        assert!(acceptable(0, &img));
        assert!(!acceptable(MAX_FILE_ATTACHMENTS, &img));
        let big = PendingAttachment {
            size: IMAGE_MAX_FILE_SIZE + 1,
            ..img.clone()
        };
        assert!(!acceptable(0, &big));
    }
}
