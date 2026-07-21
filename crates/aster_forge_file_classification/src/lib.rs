//! File extension parsing and category classification helpers.
//!
//! This crate turns filenames, MIME hints, and extension filter strings into stable high-level
//! categories used by API filtering and UI grouping. It intentionally avoids product-specific enum
//! derives so services can map categories into their own database or OpenAPI representations.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

use std::str::FromStr;

#[cfg(feature = "sea-orm")]
use sea_orm::entity::prelude::*;

/// Storage width required for persisted extension and category values.
///
/// Products persisting [`FileClassification::extension`],
/// [`FileClassification::compound_extension`], or [`FileCategory`] should use a string column with
/// at least this width. Increasing this value is a schema compatibility change for consumers.
pub const FILE_CLASSIFICATION_STORAGE_LEN: u32 = 32;
/// Maximum accepted extension filter length.
pub const MAX_EXTENSION_LEN: usize = FILE_CLASSIFICATION_STORAGE_LEN as usize;
/// Maximum number of extension filters accepted in one filter string.
pub const MAX_EXTENSION_FILTERS: usize = 32;

/// Result type returned by file classification helpers.
pub type Result<T> = std::result::Result<T, FileClassificationError>;

/// Error returned when extension or category parsing fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct FileClassificationError {
    message: String,
}

impl FileClassificationError {
    /// Creates a classification error with a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the stored error message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// High-level file category inferred from extension and MIME type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
#[cfg_attr(feature = "sea-orm", derive(EnumIter, DeriveActiveEnum))]
#[cfg_attr(
    feature = "sea-orm",
    sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")
)]
#[serde(rename_all = "lowercase")]
pub enum FileCategory {
    /// Image files.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "image"))]
    Image,
    /// Video files.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "video"))]
    Video,
    /// Audio files.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "audio"))]
    Audio,
    /// Document and plain text files.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "document"))]
    Document,
    /// Spreadsheet files.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "spreadsheet"))]
    Spreadsheet,
    /// Presentation files.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "presentation"))]
    Presentation,
    /// Archive and compressed files.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "archive"))]
    Archive,
    /// Source code and structured text files.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "code"))]
    Code,
    /// Files that do not match a known category.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "other"))]
    Other,
}

impl FileCategory {
    /// Returns the lowercase stable string representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Document => "document",
            Self::Spreadsheet => "spreadsheet",
            Self::Presentation => "presentation",
            Self::Archive => "archive",
            Self::Code => "code",
            Self::Other => "other",
        }
    }
}

impl FromStr for FileCategory {
    type Err = ();

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "image" => Ok(Self::Image),
            "video" => Ok(Self::Video),
            "audio" => Ok(Self::Audio),
            "document" => Ok(Self::Document),
            "spreadsheet" => Ok(Self::Spreadsheet),
            "presentation" => Ok(Self::Presentation),
            "archive" => Ok(Self::Archive),
            "code" => Ok(Self::Code),
            "other" => Ok(Self::Other),
            _ => Err(()),
        }
    }
}

const COMPOUND_EXTENSIONS: &[&str] = &[
    "tar.gz", "tar.bz2", "tar.xz", "tar.zst", "tar.br", "tar.lz", "tar.lzma", "tar.lzo",
];

const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "svg", "ico", "avif", "heic",
    "heif", "raw", "cr2", "nef", "orf", "rw2",
];

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "avi", "mkv", "webm", "flv", "wmv", "mpeg", "mpg", "3gp", "ts", "m2ts",
    "ogv",
];

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "flac", "aac", "m4a", "ogg", "oga", "opus", "wma", "aiff", "alac", "mid", "midi",
];

const DOCUMENT_EXTENSIONS: &[&str] = &[
    "pdf", "txt", "md", "markdown", "rtf", "doc", "docx", "odt", "pages", "epub", "tex",
];

const SPREADSHEET_EXTENSIONS: &[&str] = &["xls", "xlsx", "ods", "csv", "tsv", "numbers"];

const PRESENTATION_EXTENSIONS: &[&str] = &["ppt", "pptx", "odp", "key"];

const ARCHIVE_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "zst", "br", "tgz", "tbz", "tbz2", "txz", "lz",
    "lzma", "lzo", "cab", "iso", "dmg",
];

const CODE_EXTENSIONS: &[&str] = &[
    "rs",
    "ts",
    "tsx",
    "js",
    "jsx",
    "mjs",
    "cjs",
    "json",
    "jsonc",
    "yaml",
    "yml",
    "toml",
    "xml",
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    "sql",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "py",
    "rb",
    "go",
    "java",
    "kt",
    "kts",
    "swift",
    "c",
    "h",
    "cpp",
    "cc",
    "cxx",
    "hpp",
    "cs",
    "php",
    "lua",
    "dart",
    "vue",
    "svelte",
    "lock",
    "ini",
    "conf",
    "dockerfile",
    "makefile",
];

/// Parsed classification details for a file name and MIME type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileClassification {
    /// Lowercase final extension, without a leading dot.
    pub extension: String,
    /// Recognized compound extension, such as `tar.gz`.
    pub compound_extension: Option<String>,
    /// Inferred high-level file category.
    pub category: FileCategory,
}

/// Classifies a file from its name and MIME type.
pub fn classify_file(name: &str, mime_type: &str) -> FileClassification {
    let extension = extension_from_name(name).unwrap_or_default();
    let compound_extension = compound_extension_from_name(name);
    let category =
        classify_extension_and_mime(&extension, compound_extension.as_deref(), mime_type);

    FileClassification {
        extension,
        compound_extension,
        category,
    }
}

/// Normalizes one extension filter value.
pub fn normalize_extension_filter(raw: &str) -> Result<String> {
    let normalized = raw.trim().trim_start_matches('.').to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(FileClassificationError::new(
            "extensions must not contain empty values",
        ));
    }
    if normalized.len() > MAX_EXTENSION_LEN {
        return Err(FileClassificationError::new(format!(
            "extensions must be at most {MAX_EXTENSION_LEN} characters"
        )));
    }
    if normalized.starts_with('.')
        || normalized.ends_with('.')
        || normalized.contains("..")
        || !normalized.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' || ch == '+'
        })
    {
        return Err(FileClassificationError::new(
            "extensions may only contain letters, numbers, dot, underscore, plus, or hyphen",
        ));
    }

    Ok(normalized)
}

/// Parses a comma-separated list of extension filters.
pub fn parse_extension_filters(raw: &str) -> Result<Vec<String>> {
    let mut extensions = Vec::new();
    for part in raw.split(',') {
        let extension = normalize_extension_filter(part)?;
        if !extensions.iter().any(|candidate| candidate == &extension) {
            extensions.push(extension);
        }
        if extensions.len() > MAX_EXTENSION_FILTERS {
            return Err(FileClassificationError::new(format!(
                "extensions supports at most {MAX_EXTENSION_FILTERS} values"
            )));
        }
    }

    Ok(extensions)
}

/// Parses a file category from its lowercase string representation.
pub fn parse_file_category(raw: &str) -> Result<FileCategory> {
    FileCategory::from_str(raw.trim()).map_err(|()| {
        FileClassificationError::new(
            "category must be one of: image, video, audio, document, spreadsheet, presentation, archive, code, other",
        )
    })
}

/// Extracts the lowercase final extension from a file name.
///
/// Only ASCII-alphanumeric candidates count as extensions; path-like input
/// (`"dir.ext/file"`) or names whose suffix contains spaces/punctuation return
/// `None`, because the extracted value can be persisted and shown in UIs.
pub fn extension_from_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    let dot = trimmed.rfind('.')?;
    if dot == 0 || dot + 1 >= trimmed.len() {
        return None;
    }
    let extension = &trimmed[dot + 1..];
    if extension.is_empty()
        || extension.len() > MAX_EXTENSION_LEN
        || !extension.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(extension.to_ascii_lowercase())
}

/// Extracts a recognized compound extension from a file name.
pub fn compound_extension_from_name(name: &str) -> Option<String> {
    let normalized = name.trim().to_ascii_lowercase();
    COMPOUND_EXTENSIONS
        .iter()
        .find(|extension| normalized.ends_with(&format!(".{extension}")))
        .map(|extension| (*extension).to_string())
}

fn classify_extension_and_mime(
    extension: &str,
    compound_extension: Option<&str>,
    mime_type: &str,
) -> FileCategory {
    if compound_extension.is_some() || contains(ARCHIVE_EXTENSIONS, extension) {
        return FileCategory::Archive;
    }
    if contains(SPREADSHEET_EXTENSIONS, extension) {
        return FileCategory::Spreadsheet;
    }
    if contains(PRESENTATION_EXTENSIONS, extension) {
        return FileCategory::Presentation;
    }
    if contains(IMAGE_EXTENSIONS, extension) {
        return FileCategory::Image;
    }
    if contains(VIDEO_EXTENSIONS, extension) {
        return FileCategory::Video;
    }
    if contains(AUDIO_EXTENSIONS, extension) {
        return FileCategory::Audio;
    }
    if contains(DOCUMENT_EXTENSIONS, extension) {
        return FileCategory::Document;
    }
    if contains(CODE_EXTENSIONS, extension) {
        return FileCategory::Code;
    }

    classify_mime(mime_type)
}

fn classify_mime(mime_type: &str) -> FileCategory {
    let mime = mime_type.trim().to_ascii_lowercase();
    if mime.starts_with("image/") {
        FileCategory::Image
    } else if mime.starts_with("video/") {
        FileCategory::Video
    } else if mime.starts_with("audio/") {
        FileCategory::Audio
    } else if mime.contains("spreadsheet") || mime.contains("excel") || mime.ends_with("/csv") {
        // This must precede the generic `text/` branch: `text/csv` starts
        // with `text/` and would otherwise never reach the `/csv` check.
        FileCategory::Spreadsheet
    } else if mime == "application/pdf" || mime.starts_with("text/") {
        FileCategory::Document
    } else if mime.contains("presentation") || mime.contains("powerpoint") {
        FileCategory::Presentation
    } else if mime.contains("zip")
        || mime.contains("compressed")
        || mime.contains("x-tar")
        || mime.contains("x-7z")
        || mime.contains("x-rar")
    {
        FileCategory::Archive
    } else if mime.contains("json") || mime.contains("xml") {
        FileCategory::Code
    } else {
        FileCategory::Other
    }
}

const fn contains(values: &[&str], needle: &str) -> bool {
    let mut index = 0;
    while index < values.len() {
        if values[index].len() == needle.len() {
            let a = values[index].as_bytes();
            let b = needle.as_bytes();
            let mut byte_index = 0;
            let mut equal = true;
            while byte_index < a.len() {
                if a[byte_index] != b[byte_index] {
                    equal = false;
                    break;
                }
                byte_index += 1;
            }
            if equal {
                return true;
            }
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extensions_and_compound_extensions() {
        assert_eq!(extension_from_name("backup.tar.gz").as_deref(), Some("gz"));
        assert_eq!(
            compound_extension_from_name("backup.TAR.GZ").as_deref(),
            Some("tar.gz")
        );
        assert_eq!(extension_from_name(".gitignore"), None);
        assert_eq!(extension_from_name("README"), None);
        assert_eq!(
            extension_from_name(&format!("file.{}", "a".repeat(33))),
            None
        );
    }

    #[test]
    fn classifies_with_fixed_priority() {
        let csv = classify_file("data.csv", "text/csv");
        assert_eq!(csv.category, FileCategory::Spreadsheet);

        let markdown = classify_file("README.md", "text/markdown");
        assert_eq!(markdown.category, FileCategory::Document);

        let archive = classify_file("backup.tar.gz", "application/gzip");
        assert_eq!(archive.category, FileCategory::Archive);
        assert_eq!(archive.compound_extension.as_deref(), Some("tar.gz"));
    }

    #[test]
    fn classifies_from_mime_when_extension_is_unknown() {
        assert_eq!(
            classify_file("asset.unknown", "image/png").category,
            FileCategory::Image
        );
        assert_eq!(
            classify_file("asset.unknown", "application/vnd.ms-excel").category,
            FileCategory::Spreadsheet
        );
        assert_eq!(
            classify_file("asset.unknown", "application/json").category,
            FileCategory::Code
        );
        assert_eq!(
            classify_file("asset.unknown", "application/octet-stream").category,
            FileCategory::Other
        );
    }

    #[test]
    fn classifies_text_csv_mime_as_spreadsheet_when_extension_is_unknown() {
        // The spreadsheet branch must run before the generic `text/` branch,
        // or `text/csv` always classifies as Document and the `/csv` arm is
        // unreachable.
        assert_eq!(
            classify_file("data.unknown", "text/csv").category,
            FileCategory::Spreadsheet
        );
        assert_eq!(
            classify_file("notes.unknown", "text/markdown").category,
            FileCategory::Document
        );
    }

    #[test]
    fn extension_from_name_rejects_path_like_and_spaced_candidates() {
        // The extracted value can be persisted, so candidates that are clearly
        // not extensions must be rejected instead of leaking garbage into DB
        // rows, logs, and UI labels.
        assert_eq!(extension_from_name("dir.ext/file"), None);
        assert_eq!(extension_from_name("report.pn g"), None);
        assert_eq!(extension_from_name("archive.tar.gz").as_deref(), Some("gz"));
        assert_eq!(extension_from_name("photo.JPEG").as_deref(), Some("jpeg"));
    }

    #[test]
    fn parses_file_category_values() {
        assert_eq!(parse_file_category(" image ").unwrap(), FileCategory::Image);
        assert_eq!(FileCategory::Archive.as_str(), "archive");
        assert!(parse_file_category("folder").is_err());
    }

    #[test]
    fn normalizes_extension_filters() {
        assert_eq!(
            parse_extension_filters(" .PDF,docx,pdf ").unwrap(),
            vec!["pdf", "docx"]
        );
        assert!(parse_extension_filters("pdf,,docx").is_err());
        assert!(parse_extension_filters("../pdf").is_err());
    }

    #[test]
    fn extension_filters_reject_length_and_count_boundaries() {
        assert!(normalize_extension_filter(&"a".repeat(MAX_EXTENSION_LEN + 1)).is_err());

        let too_many = (0..=MAX_EXTENSION_FILTERS)
            .map(|index| format!("ext{index}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_extension_filters(&too_many).is_err());
    }

    #[cfg(feature = "sea-orm")]
    #[test]
    fn file_category_has_stable_sea_orm_values() {
        assert_eq!(FileCategory::Image.to_value(), "image");
        assert_eq!(FileCategory::Archive.to_value(), "archive");
        assert_eq!(
            FileCategory::try_from_value(&"spreadsheet".to_string()),
            Ok(FileCategory::Spreadsheet)
        );
        assert!(FileCategory::try_from_value(&"folder".to_string()).is_err());
        for category in [
            FileCategory::Image,
            FileCategory::Video,
            FileCategory::Audio,
            FileCategory::Document,
            FileCategory::Spreadsheet,
            FileCategory::Presentation,
            FileCategory::Archive,
            FileCategory::Code,
            FileCategory::Other,
        ] {
            assert!(category.to_value().len() <= FILE_CLASSIFICATION_STORAGE_LEN as usize);
        }
    }
}
