//! Image parts for a model turn.
//!
//! A turn can carry an image without making the text contract heavier: the message keeps its
//! text `content`, and the image URLs travel beside it. An OpenAI-compatible provider receives
//! them as the standard `text` / `image_url` content array.
//!
//! A local image is read only when the caller names it, must live under the project root, and is
//! inlined as a `data:` URL. A remote image stays a URL for the provider to fetch. This module
//! never walks the tree looking for images.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The largest local image this build inlines. The room attachment reader uses the same cap; a
/// larger image is refused instead of being silently truncated.
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// A base64 data URL grows by four thirds, plus the media-type prefix.
const MAX_DATA_URL_CHARS: usize = MAX_IMAGE_BYTES.div_ceil(3) * 4 + 64;

/// One image attached to a user message. `url` is either an HTTPS URL or a `data:` URL built
/// from bytes under the project root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePart {
    pub url: String,
}

impl ImagePart {
    /// Resolve one reference from a CLI flag, HTTP body or desktop attachment. A remote image
    /// must be HTTPS; a local path is resolved against the project and may not escape it.
    pub fn from_reference(project_root: &Path, reference: &str) -> Result<Self, String> {
        let reference = reference.trim();
        if reference.is_empty() {
            return Err("an image reference is empty".to_string());
        }

        if reference.starts_with("https://") {
            if reference.len() <= "https://".len() {
                return Err("an image URL has no host".to_string());
            }
            return Ok(Self {
                url: reference.to_string(),
            });
        }

        if reference.starts_with("data:image/") {
            return Self::from_data_url(reference);
        }

        if reference.contains("://") {
            return Err("an image must be an HTTPS URL or a path inside this project".to_string());
        }

        let root = project_root
            .canonicalize()
            .map_err(|error| format!("could not open the project root: {error}"))?;
        let candidate = Path::new(reference);
        let path = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        };
        let path = path
            .canonicalize()
            .map_err(|error| format!("could not open image {reference}: {error}"))?;
        if !path.starts_with(&root) {
            return Err(format!("image {reference} is outside this project"));
        }
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("could not read image {reference}: {error}"))?;
        if !metadata.is_file() {
            return Err(format!("image {reference} is not a file"));
        }
        if metadata.len() > MAX_IMAGE_BYTES as u64 {
            return Err(format!(
                "image {reference} is larger than {} MB",
                MAX_IMAGE_BYTES / (1024 * 1024)
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("could not read image {reference}: {error}"))?;
        let mime = image_mime(&bytes).ok_or_else(|| {
            format!("{reference} is not a PNG, JPEG, GIF, WebP, AVIF or BMP image")
        })?;
        Ok(Self {
            url: format!("data:{mime};base64,{}", base64(&bytes)),
        })
    }

    fn from_data_url(reference: &str) -> Result<Self, String> {
        if reference.len() > MAX_DATA_URL_CHARS {
            return Err(format!(
                "the inline image is larger than {} MB",
                MAX_IMAGE_BYTES / (1024 * 1024)
            ));
        }
        let Some((meta, payload)) = reference.split_once(";base64,") else {
            return Err("an inline image must be base64-encoded".to_string());
        };
        let mime = meta
            .strip_prefix("data:")
            .ok_or_else(|| "an inline image must start with data:image/".to_string())?;
        if !matches!(
            mime,
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif" | "image/bmp"
        ) {
            return Err(format!("{mime} is not a supported image type"));
        }
        if payload.is_empty() {
            return Err("the inline image has no bytes".to_string());
        }
        Ok(Self {
            url: reference.to_string(),
        })
    }
}

/// Identify an image by its leading bytes, not its name. A text file renamed to `.png` is still
/// text and is refused before it can be sent to a provider.
pub fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.len() >= 12 && &bytes[4..12] == b"ftypavif" {
        return Some("image/avif");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }
    None
}

/// Standard base64 with padding. The same small encoder as the room attachment reader; keeping
/// it here lets the turn contract build a data URL without another dependency.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let packed = ((chunk[0] as u32) << 16) | ((second as u32) << 8) | third as u32;
        out.push(TABLE[(packed >> 18) as usize & 63] as char);
        out.push(TABLE[(packed >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(packed >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[packed as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

    #[test]
    fn a_project_image_is_inlined_as_a_data_url() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".rigel/attachments")).unwrap();
        std::fs::write(project.path().join(".rigel/attachments/sign.png"), PNG).unwrap();

        let part = ImagePart::from_reference(project.path(), ".rigel/attachments/sign.png")
            .expect("the attached image should load");

        assert!(part.url.starts_with("data:image/png;base64,"));
        assert!(
            part.url.ends_with("iVBORw0KGgoAAAANSUhEUg=="),
            "{}",
            part.url
        );
    }

    #[test]
    fn a_file_extension_cannot_make_text_an_image() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("sign.png"), "not an image").unwrap();
        let error = ImagePart::from_reference(project.path(), "sign.png").unwrap_err();
        assert!(error.contains("is not a PNG"), "{error}");
    }

    #[test]
    fn a_path_outside_the_project_is_refused() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("sign.png");
        std::fs::write(&path, PNG).unwrap();

        let error = ImagePart::from_reference(project.path(), &path.to_string_lossy()).unwrap_err();
        assert!(error.contains("outside this project"), "{error}");

        let error = ImagePart::from_reference(project.path(), "../sign.png").unwrap_err();
        assert!(error.contains("could not open image"), "{error}");
    }

    #[test]
    fn a_remote_url_uses_https_and_never_a_local_scheme() {
        let part =
            ImagePart::from_reference(Path::new("."), "https://example.com/traffic-sign.png")
                .unwrap();
        assert_eq!(part.url, "https://example.com/traffic-sign.png");

        for bad in ["http://example.com/a.png", "file:///C:/secret.png"] {
            let error = ImagePart::from_reference(Path::new("."), bad).unwrap_err();
            assert!(error.contains("HTTPS URL"), "{bad}: {error}");
        }
    }

    #[test]
    fn an_inline_image_must_be_a_supported_base64_type() {
        let part = ImagePart::from_reference(Path::new("."), "data:image/png;base64,iVBORw0KGgo=")
            .unwrap();
        assert_eq!(part.url, "data:image/png;base64,iVBORw0KGgo=");

        for bad in [
            "data:image/svg+xml;base64,PHN2Zz4=",
            "data:image/png,not-base64",
            "data:image/png;base64,",
        ] {
            assert!(
                ImagePart::from_reference(Path::new("."), bad).is_err(),
                "{bad} should be refused"
            );
        }
    }

    #[test]
    fn mime_is_read_from_the_bytes() {
        assert_eq!(image_mime(PNG), Some("image/png"));
        assert_eq!(image_mime(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(image_mime(b"GIF89a"), Some("image/gif"));
        assert_eq!(
            image_mime(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
            Some("image/webp")
        );
        assert_eq!(image_mime(b"\x00\x00\x00\x00ftypavif"), Some("image/avif"));
        assert_eq!(image_mime(b"BM"), Some("image/bmp"));
        assert_eq!(image_mime(b"PK\x03\x04"), None);
    }
}
