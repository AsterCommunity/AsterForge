//! RFC 8144 `WebDAV` preference selection from a validated capability snapshot.

use http::HeaderValue;
use http::header::HeaderMap;

use crate::{DavCapabilitySnapshot, DavExtensionPackage, DavMethod, DavPreferenceSet, Depth};

/// Applicable RFC 8144 preferences for one request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DavPreferencePlan {
    pub return_minimal: bool,
    pub return_representation: bool,
    pub depth_no_root: bool,
}

impl DavPreferencePlan {
    /// Canonical `Preference-Applied` value for the subset actually honored by the response.
    ///
    /// Request planning only determines eligibility. The response composer must pass a set that
    /// reflects actual behavior; for example, a failed PROPPATCH must not claim that
    /// `return=minimal` was applied, and `return=representation` is applied only when a current
    /// representation is returned.
    #[must_use]
    pub fn preference_applied_header(&self, applied: DavPreferenceSet) -> Option<HeaderValue> {
        applied_header(
            self.return_minimal && applied.contains(DavPreferenceSet::RETURN_MINIMAL),
            self.return_representation && applied.contains(DavPreferenceSet::RETURN_REPRESENTATION),
            self.depth_no_root && applied.contains(DavPreferenceSet::DEPTH_NO_ROOT),
        )
    }
}

/// Selects supported and method-applicable RFC 8144 preferences.
///
/// Unknown preferences, parameters, and values are ignored as requested by the generic Prefer
/// framework. Parsing and canonical response-header selection do not allocate.
#[must_use]
pub fn plan_preferences(
    snapshot: &DavCapabilitySnapshot,
    headers: &HeaderMap,
    method: DavMethod,
    depth: Option<Depth>,
) -> DavPreferencePlan {
    if !snapshot.supports_extension(DavExtensionPackage::Prefer) || !snapshot.allows(method) {
        return DavPreferencePlan::default();
    }

    let requested = requested_preferences(headers);
    let return_minimal =
        requested.contains(DavPreferenceSet::RETURN_MINIMAL) && minimal_applies(snapshot, method);
    let return_representation = requested.contains(DavPreferenceSet::RETURN_REPRESENTATION)
        && matches!(
            method,
            DavMethod::Put | DavMethod::Copy | DavMethod::Move | DavMethod::Patch | DavMethod::Post
        );
    let depth_no_root = requested.contains(DavPreferenceSet::DEPTH_NO_ROOT)
        && method_supports_depth(method)
        && matches!(depth, Some(Depth::One | Depth::Infinity));

    DavPreferencePlan {
        return_minimal,
        return_representation,
        depth_no_root,
    }
}

fn requested_preferences(headers: &HeaderMap) -> DavPreferenceSet {
    let mut requested = DavPreferenceSet::empty();
    for value in headers.get_all("Prefer") {
        let Ok(value) = value.to_str() else {
            continue;
        };
        let mut start = 0;
        let mut quoted = false;
        let mut escaped = false;
        for (index, byte) in value.bytes().enumerate() {
            if escaped {
                escaped = false;
            } else if quoted && byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = !quoted;
            } else if byte == b',' && !quoted {
                add_requested_preference(&value[start..index], &mut requested);
                start = index + 1;
            }
        }
        add_requested_preference(&value[start..], &mut requested);
    }
    requested
}

fn add_requested_preference(value: &str, requested: &mut DavPreferenceSet) {
    let mut quoted = false;
    let mut escaped = false;
    let mut end = value.len();
    for (index, byte) in value.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if quoted && byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            quoted = !quoted;
        } else if byte == b';' && !quoted {
            end = index;
            break;
        }
    }
    let preference = value[..end].trim();
    if preference.eq_ignore_ascii_case("return=minimal") {
        *requested = requested.union(DavPreferenceSet::RETURN_MINIMAL);
    } else if preference.eq_ignore_ascii_case("return=representation") {
        *requested = requested.union(DavPreferenceSet::RETURN_REPRESENTATION);
    } else if preference.eq_ignore_ascii_case("depth-noroot") {
        *requested = requested.union(DavPreferenceSet::DEPTH_NO_ROOT);
    }
}

fn minimal_applies(snapshot: &DavCapabilitySnapshot, method: DavMethod) -> bool {
    matches!(
        method,
        DavMethod::Propfind | DavMethod::Proppatch | DavMethod::Report
    ) || (method == DavMethod::Mkcol
        && snapshot.supports_extension(DavExtensionPackage::ExtendedMkcol))
}

const fn method_supports_depth(method: DavMethod) -> bool {
    matches!(
        method,
        DavMethod::Propfind
            | DavMethod::Delete
            | DavMethod::Copy
            | DavMethod::Move
            | DavMethod::Lock
            | DavMethod::Report
    )
}

fn applied_header(
    return_minimal: bool,
    return_representation: bool,
    depth_no_root: bool,
) -> Option<HeaderValue> {
    let value = match (return_minimal, return_representation, depth_no_root) {
        (false, false, false) => return None,
        (true, false, false) => "return=minimal",
        (false, true, false) => "return=representation",
        (false, false, true) => "depth-noroot",
        (true, true, false) => "return=minimal, return=representation",
        (true, false, true) => "return=minimal, depth-noroot",
        (false, true, true) => "return=representation, depth-noroot",
        (true, true, true) => "return=minimal, return=representation, depth-noroot",
    };
    Some(HeaderValue::from_static(value))
}
