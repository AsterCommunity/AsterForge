//! Canonical WebDAV path handling.

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

const DAV_HREF_PATH_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'&')
    .add(b'\'')
    .add(b'+')
    .add(b'%');

/// A normalized path relative to a WebDAV mount.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DavPath {
    raw: String,
    decoded: Vec<u8>,
}

/// Parses a mount-relative request path and returns its canonical decoded representation.
pub fn decode_relative_path(relative: &str) -> Result<(DavPath, String), DavPathError> {
    let path = DavPath::new(relative)?;
    let decoded = path.as_str().to_string();
    Ok((path, decoded))
}

/// Percent-encodes a DAV href while preserving path separators.
#[must_use]
pub fn encode_href(path: &str) -> String {
    utf8_percent_encode(path, DAV_HREF_PATH_SET).to_string()
}

/// Builds an encoded href from a mount prefix and decoded relative path.
#[must_use]
pub fn href_for_relative(prefix: &str, relative: &str) -> String {
    let href = if relative == "/" {
        format!("{prefix}/")
    } else {
        format!("{prefix}{relative}")
    };
    encode_href(&href)
}

/// Builds an encoded href from a mount prefix and canonical DAV path.
#[must_use]
pub fn href_for_dav_path(prefix: &str, path: &DavPath) -> String {
    href_for_relative(prefix, path.as_str())
}

/// Returns a child path with collection trailing-slash semantics.
#[must_use]
pub fn child_relative_path(parent: &str, name: &[u8], is_collection: bool) -> String {
    let name = String::from_utf8_lossy(name);
    let mut relative = if parent == "/" {
        format!("/{name}")
    } else if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    };
    if is_collection && !relative.ends_with('/') {
        relative.push('/');
    }
    relative
}

/// Returns the canonical parent collection path.
#[must_use]
pub fn parent_relative_path(relative: &str) -> Option<String> {
    if relative == "/" {
        return None;
    }
    let trimmed = relative.trim_end_matches('/');
    let mut segments = trimmed
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() <= 1 {
        return Some("/".to_string());
    }
    segments.pop();
    Some(format!("/{}/", segments.join("/")))
}

/// Returns the final decoded segment for DAV display-name generation.
#[must_use]
pub fn display_name(relative: &str) -> &str {
    if relative == "/" {
        ""
    } else {
        relative
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
    }
}

/// Errors produced while canonicalizing a WebDAV path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavPathError {
    /// The path contains malformed percent encoding.
    #[error("invalid WebDAV path encoding")]
    InvalidEncoding,
    /// Dot-segment normalization would escape the WebDAV mount root.
    #[error("WebDAV path escapes the mount root")]
    PathEscape,
}

impl DavPath {
    /// Percent-decodes and canonicalizes a path without allowing root escape.
    pub fn new(path: &str) -> Result<Self, DavPathError> {
        let raw = ensure_leading_slash(path);
        let decoded = urlencoding::decode(&raw)
            .map_err(|_| DavPathError::InvalidEncoding)?
            .into_owned();
        let raw = clean_decoded_path(&decoded)?;
        let decoded = raw.as_bytes().to_vec();
        Ok(Self { raw, decoded })
    }

    /// Returns the WebDAV mount root.
    #[must_use]
    pub fn root() -> Self {
        Self {
            raw: "/".to_string(),
            decoded: b"/".to_vec(),
        }
    }

    /// Returns the decoded canonical path bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.decoded
    }

    /// Returns the decoded canonical UTF-8 path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Returns whether the path denotes a collection alias.
    #[must_use]
    pub fn is_collection(&self) -> bool {
        self.raw == "/" || self.raw.ends_with('/')
    }

    /// Returns the decoded canonical path as a relative mount path.
    #[must_use]
    pub fn relative(&self) -> &str {
        self.as_str()
    }
}

fn ensure_leading_slash(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return "/".to_string();
    }

    let mut normalized = path.to_string();
    if !normalized.starts_with('/') {
        normalized.insert(0, '/');
    }
    normalized
}

fn clean_decoded_path(path: &str) -> Result<String, DavPathError> {
    let mut segments = Vec::new();
    let mut is_collection = false;

    for (index, segment) in path.split('/').enumerate() {
        match segment {
            "" => {
                if index > 0 {
                    is_collection = true;
                }
            }
            "." => is_collection = true,
            ".." => {
                if segments.pop().is_none() {
                    return Err(DavPathError::PathEscape);
                }
                is_collection = true;
            }
            segment => {
                segments.push(segment);
                is_collection = false;
            }
        }
    }

    if segments.is_empty() {
        return Ok("/".to_string());
    }

    let mut cleaned = format!("/{}", segments.join("/"));
    if is_collection && !cleaned.ends_with('/') {
        cleaned.push('/');
    }
    Ok(cleaned)
}
