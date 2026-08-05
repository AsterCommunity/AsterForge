//! Canonical `WebDAV` path handling.

use std::str;

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};

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

/// A normalized path relative to a `WebDAV` mount.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DavPath {
    canonical: String,
}

/// Parses a mount-relative request path and returns its canonical decoded representation.
///
/// # Errors
///
/// Returns [`DavPathError`] when percent-decoding fails or dot segments escape the mount.
pub fn decode_relative_path(relative: &str) -> Result<DavPath, DavPathError> {
    DavPath::new(relative)
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
///
/// # Errors
///
/// Returns [`DavPathError`] when the child name is invalid or escapes the parent path.
pub fn child_relative_path(
    parent: &str,
    name: &[u8],
    is_collection: bool,
) -> Result<String, DavPathError> {
    let name = str::from_utf8(name).map_err(|_| DavPathError::InvalidEncoding)?;
    if name.is_empty() || matches!(name, "." | "..") || name.contains(['/', '\\']) {
        return Err(DavPathError::InvalidChildName);
    }
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
    Ok(relative)
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

/// Errors produced while canonicalizing a `WebDAV` path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavPathError {
    /// The path contains malformed percent encoding.
    #[error("invalid WebDAV path encoding")]
    InvalidEncoding,
    /// Dot-segment normalization would escape the `WebDAV` mount root.
    #[error("WebDAV path escapes the mount root")]
    PathEscape,
    /// A backend child name is empty, a dot segment, or contains a path separator.
    #[error("invalid WebDAV child name")]
    InvalidChildName,
}

impl DavPath {
    /// Percent-decodes and canonicalizes a path without allowing root escape.
    ///
    /// # Errors
    ///
    /// Returns [`DavPathError`] when the mount path is invalid or the URI escapes that mount.
    pub fn new(path: &str) -> Result<Self, DavPathError> {
        let encoded = ensure_leading_slash(path);
        if contains_encoded_path_separator(&encoded) {
            return Err(DavPathError::InvalidEncoding);
        }
        let decoded = percent_decode_str(&encoded)
            .decode_utf8()
            .map_err(|_| DavPathError::InvalidEncoding)?;
        let canonical = clean_decoded_path(&decoded)?;
        Ok(Self { canonical })
    }

    /// Returns the `WebDAV` mount root.
    #[must_use]
    pub fn root() -> Self {
        Self {
            canonical: "/".to_string(),
        }
    }

    /// Returns the decoded canonical path bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.canonical.as_bytes()
    }

    /// Returns the decoded canonical UTF-8 path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.canonical
    }

    /// Returns whether the path denotes a collection alias.
    #[must_use]
    pub fn is_collection(&self) -> bool {
        self.canonical == "/" || self.canonical.ends_with('/')
    }

    /// Returns the canonical parent collection without reparsing decoded path data.
    ///
    /// Returns `None` when this path is the `WebDAV` mount root.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        parent_relative_path(&self.canonical).map(|canonical| Self { canonical })
    }

    /// Joins one decoded backend child name without treating literal percent bytes as URI input.
    ///
    /// # Errors
    ///
    /// Returns [`DavPathError`] when the decoded child name is not UTF-8, is empty, is a dot
    /// segment, or contains a path separator.
    pub fn join_child(
        &self,
        decoded_name: &[u8],
        is_collection: bool,
    ) -> Result<Self, DavPathError> {
        let canonical = child_relative_path(&self.canonical, decoded_name, is_collection)?;
        Ok(Self { canonical })
    }
}

fn contains_encoded_path_separator(path: &str) -> bool {
    path.as_bytes().windows(3).any(|window| {
        let high = window[1].to_ascii_lowercase();
        let low = window[2].to_ascii_lowercase();
        window[0] == b'%' && matches!((high, low), (b'2', b'f') | (b'5', b'c'))
    })
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
