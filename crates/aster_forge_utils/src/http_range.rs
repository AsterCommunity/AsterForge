//! Transport-neutral parsing for a single HTTP byte range.

/// A resolved inclusive byte range for one representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpByteRange {
    start: u64,
    end: u64,
    length: u64,
    total_size: u64,
}

impl HttpByteRange {
    /// Creates a resolved byte range and validates it against the representation length.
    pub fn new(start: u64, end: u64, total_size: u64) -> Result<Self, HttpRangeError> {
        if total_size == 0 {
            return Err(HttpRangeError::EmptyRepresentation);
        }
        if start > end || end >= total_size {
            return Err(HttpRangeError::Unsatisfiable);
        }
        Ok(Self {
            start,
            end,
            length: end - start + 1,
            total_size,
        })
    }

    #[must_use]
    pub const fn start(self) -> u64 {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> u64 {
        self.end
    }

    #[must_use]
    pub const fn length(self) -> u64 {
        self.length
    }

    #[must_use]
    pub const fn total_size(self) -> u64 {
        self.total_size
    }

    /// Renders the value required by a successful `Content-Range` response header.
    #[must_use]
    pub fn content_range_header(self) -> String {
        format!("bytes {}-{}/{}", self.start, self.end, self.total_size)
    }
}

/// Stable failure categories for a single byte-range request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HttpRangeError {
    #[error("range header must use the bytes unit")]
    UnsupportedUnit,
    #[error("multiple range requests are not supported")]
    MultipleRangesUnsupported,
    #[error("range header is malformed")]
    Malformed,
    #[error("range bound must be a valid unsigned integer")]
    InvalidNumber,
    #[error("range cannot be requested for an empty representation")]
    EmptyRepresentation,
    #[error("range is not satisfiable for the current representation")]
    Unsatisfiable,
}

/// Parses and resolves one RFC byte-range specifier against a representation length.
///
/// Multiple ranges are reported separately so callers can choose whether to reject them or
/// implement multipart responses. End bounds beyond the representation are clamped as required
/// by HTTP range semantics.
pub fn parse_single_byte_range(
    raw: &str,
    total_size: u64,
) -> Result<HttpByteRange, HttpRangeError> {
    let range = raw
        .strip_prefix("bytes=")
        .ok_or(HttpRangeError::UnsupportedUnit)?;
    if range.contains(',') {
        return Err(HttpRangeError::MultipleRangesUnsupported);
    }

    let (start_raw, end_raw) = range.split_once('-').ok_or(HttpRangeError::Malformed)?;
    if start_raw.is_empty() && end_raw.is_empty() {
        return Err(HttpRangeError::Malformed);
    }
    if total_size == 0 {
        return Err(HttpRangeError::EmptyRepresentation);
    }

    if start_raw.is_empty() {
        let suffix_length = parse_bound(end_raw)?;
        if suffix_length == 0 {
            return Err(HttpRangeError::Unsatisfiable);
        }
        let length = suffix_length.min(total_size);
        return HttpByteRange::new(total_size - length, total_size - 1, total_size);
    }

    let start = parse_bound(start_raw)?;
    if start >= total_size {
        return Err(HttpRangeError::Unsatisfiable);
    }
    let end = if end_raw.is_empty() {
        total_size - 1
    } else {
        parse_bound(end_raw)?
    };
    if end < start {
        return Err(HttpRangeError::Unsatisfiable);
    }
    HttpByteRange::new(start, end.min(total_size - 1), total_size)
}

fn parse_bound(value: &str) -> Result<u64, HttpRangeError> {
    value
        .parse::<u64>()
        .map_err(|_| HttpRangeError::InvalidNumber)
}

#[cfg(test)]
mod tests {
    use super::{HttpByteRange, HttpRangeError, parse_single_byte_range};

    #[test]
    fn resolves_bounded_open_and_suffix_ranges() {
        assert_eq!(
            parse_single_byte_range("bytes=5-9", 20),
            HttpByteRange::new(5, 9, 20)
        );
        assert_eq!(
            parse_single_byte_range("bytes=7-", 20),
            HttpByteRange::new(7, 19, 20)
        );
        assert_eq!(
            parse_single_byte_range("bytes=-6", 20),
            HttpByteRange::new(14, 19, 20)
        );
        assert_eq!(
            parse_single_byte_range("bytes=-50", 20),
            HttpByteRange::new(0, 19, 20)
        );
    }

    #[test]
    fn clamps_end_beyond_the_representation() {
        assert_eq!(
            parse_single_byte_range("bytes=17-99", 20),
            HttpByteRange::new(17, 19, 20)
        );
    }

    #[test]
    fn preserves_u64_boundaries_without_overflow() {
        let total_size = u64::MAX;
        let range = parse_single_byte_range("bytes=0-18446744073709551615", total_size)
            .expect("maximum end should clamp safely");
        assert_eq!(range.start(), 0);
        assert_eq!(range.end(), u64::MAX - 1);
        assert_eq!(range.length(), u64::MAX);
        assert_eq!(range.total_size(), total_size);
    }

    #[test]
    fn renders_content_range_and_exposes_bounds() {
        let range = HttpByteRange::new(2, 6, 10).expect("valid range");
        assert_eq!(range.start(), 2);
        assert_eq!(range.end(), 6);
        assert_eq!(range.length(), 5);
        assert_eq!(range.total_size(), 10);
        assert_eq!(range.content_range_header(), "bytes 2-6/10");
    }

    #[test]
    fn constructor_rejects_empty_inverted_and_out_of_bounds_ranges() {
        assert_eq!(
            HttpByteRange::new(0, 0, 0),
            Err(HttpRangeError::EmptyRepresentation)
        );
        assert_eq!(
            HttpByteRange::new(5, 4, 10),
            Err(HttpRangeError::Unsatisfiable)
        );
        assert_eq!(
            HttpByteRange::new(5, 10, 10),
            Err(HttpRangeError::Unsatisfiable)
        );
    }

    #[test]
    fn classifies_every_rejected_range_shape() {
        let cases = [
            ("items=0-1", HttpRangeError::UnsupportedUnit),
            ("bytes=0-1,3-4", HttpRangeError::MultipleRangesUnsupported),
            ("bytes=-", HttpRangeError::Malformed),
            ("bytes=abc-", HttpRangeError::InvalidNumber),
            ("bytes=-0", HttpRangeError::Unsatisfiable),
            ("bytes=9-5", HttpRangeError::Unsatisfiable),
            ("bytes=20-", HttpRangeError::Unsatisfiable),
        ];
        for (raw, expected) in cases {
            assert_eq!(parse_single_byte_range(raw, 20), Err(expected), "{raw}");
        }
        assert_eq!(
            parse_single_byte_range("bytes=0-0", 0),
            Err(HttpRangeError::EmptyRepresentation)
        );
    }
}
