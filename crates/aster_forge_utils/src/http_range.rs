//! Transport-neutral parsing and normalization for HTTP byte ranges.

/// A resolved inclusive byte range for one representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpByteRange {
    start: u64,
    end: u64,
    length: u64,
    total_size: u64,
}

/// A bounded, normalized byte-range set for one representation.
#[derive(Debug, PartialEq, Eq)]
pub struct HttpByteRangeSet {
    requested_count: usize,
    ranges: Vec<HttpByteRange>,
}

impl HttpByteRangeSet {
    /// Returns the number of non-empty range specs supplied by the sender.
    #[must_use]
    pub const fn requested_count(&self) -> usize {
        self.requested_count
    }

    /// Returns the satisfiable ranges in request order.
    #[must_use]
    pub fn ranges(&self) -> &[HttpByteRange] {
        &self.ranges
    }

    /// Consumes the set and returns its satisfiable ranges.
    #[must_use]
    pub fn into_ranges(self) -> Vec<HttpByteRange> {
        self.ranges
    }
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

/// Stable failure categories for byte-range requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HttpRangeError {
    #[error("range header must use the bytes unit")]
    UnsupportedUnit,
    #[error("multiple range requests are not supported")]
    MultipleRangesUnsupported,
    #[error("range request exceeds the configured number of range specs")]
    TooManyRanges,
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
    let set = parse_byte_ranges(raw, total_size, 1).map_err(|error| match error {
        HttpRangeError::TooManyRanges => HttpRangeError::MultipleRangesUnsupported,
        other => other,
    })?;
    set.into_ranges()
        .into_iter()
        .next()
        .ok_or(HttpRangeError::Unsatisfiable)
}

/// Parses an RFC 9110 `bytes` range-set with an allocation and work bound.
///
/// Empty list members are tolerated as required by the HTTP `#rule` recipient grammar. Every
/// non-empty spec must be syntactically valid, while individually unsatisfiable specs are removed
/// as long as at least one requested range remains satisfiable. End bounds beyond the current
/// representation are clamped and suffix ranges larger than the representation select it all.
pub fn parse_byte_ranges(
    raw: &str,
    total_size: u64,
    maximum_specs: usize,
) -> Result<HttpByteRangeSet, HttpRangeError> {
    let raw = raw.trim_start();
    let (unit, range_set) = raw.split_once('=').ok_or(HttpRangeError::UnsupportedUnit)?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return Err(HttpRangeError::UnsupportedUnit);
    }

    let requested_count = range_set
        .split(',')
        .filter(|spec| !spec.trim().is_empty())
        .count();
    if requested_count == 0 {
        return Err(HttpRangeError::Malformed);
    }
    if requested_count > maximum_specs {
        return Err(HttpRangeError::TooManyRanges);
    }

    let mut ranges = Vec::with_capacity(requested_count);
    for spec in range_set
        .split(',')
        .map(str::trim)
        .filter(|spec| !spec.is_empty())
    {
        if let Some(range) = parse_byte_range_spec(spec, total_size)? {
            ranges.push(range);
        }
    }
    if ranges.is_empty() {
        return Err(if total_size == 0 {
            HttpRangeError::EmptyRepresentation
        } else {
            HttpRangeError::Unsatisfiable
        });
    }

    Ok(HttpByteRangeSet {
        requested_count,
        ranges,
    })
}

fn parse_byte_range_spec(
    spec: &str,
    total_size: u64,
) -> Result<Option<HttpByteRange>, HttpRangeError> {
    let (start_raw, end_raw) = spec.split_once('-').ok_or(HttpRangeError::Malformed)?;
    if start_raw.is_empty() && end_raw.is_empty() {
        return Err(HttpRangeError::Malformed);
    }

    if start_raw.is_empty() {
        let suffix_length = parse_bound(end_raw)?;
        if suffix_length == 0 || total_size == 0 {
            return Ok(None);
        }
        let length = suffix_length.min(total_size);
        return HttpByteRange::new(total_size - length, total_size - 1, total_size).map(Some);
    }

    let start = parse_bound(start_raw)?;
    let end = if end_raw.is_empty() {
        None
    } else {
        Some(parse_bound(end_raw)?)
    };
    if end.is_some_and(|end| end < start) {
        return Err(HttpRangeError::Malformed);
    }
    if total_size == 0 || start >= total_size {
        return Ok(None);
    }
    let end = end.unwrap_or(total_size - 1).min(total_size - 1);
    HttpByteRange::new(start, end, total_size).map(Some)
}

fn parse_bound(value: &str) -> Result<u64, HttpRangeError> {
    value
        .parse::<u64>()
        .map_err(|_| HttpRangeError::InvalidNumber)
}

#[cfg(test)]
mod tests {
    use super::{HttpByteRange, HttpRangeError, parse_byte_ranges, parse_single_byte_range};

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
        assert_eq!(
            parse_single_byte_range("  BYTES=0-1", 20),
            HttpByteRange::new(0, 1, 20)
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
    fn multi_range_parser_preserves_order_and_removes_only_unsatisfiable_specs() {
        let set = parse_byte_ranges("bytes=10-12, 50-, -5, 0-4", 20, 4)
            .expect("mixed range-set should keep satisfiable specs");
        assert_eq!(set.requested_count(), 4);
        assert_eq!(
            set.ranges(),
            [
                HttpByteRange::new(10, 12, 20).expect("range"),
                HttpByteRange::new(15, 19, 20).expect("range"),
                HttpByteRange::new(0, 4, 20).expect("range"),
            ]
        );
    }

    #[test]
    fn multi_range_parser_tolerates_empty_list_members_and_clamps_suffixes() {
        let set = parse_byte_ranges("bytes=, 0-99, , -100,", 20, 2)
            .expect("empty list members are recipient-tolerated");
        assert_eq!(set.requested_count(), 2);
        assert_eq!(
            set.into_ranges(),
            vec![
                HttpByteRange::new(0, 19, 20).expect("range"),
                HttpByteRange::new(0, 19, 20).expect("range"),
            ]
        );
    }

    #[test]
    fn multi_range_parser_enforces_spec_limit_before_normalization() {
        assert_eq!(
            parse_byte_ranges("bytes=0-1,100-200", 20, 1),
            Err(HttpRangeError::TooManyRanges)
        );
        assert_eq!(
            parse_byte_ranges("bytes=100-200,300-400", 20, 2),
            Err(HttpRangeError::Unsatisfiable)
        );
        assert_eq!(
            parse_byte_ranges("bytes=-0,20-", 20, 2),
            Err(HttpRangeError::Unsatisfiable)
        );
    }

    #[test]
    fn multi_range_parser_rejects_invalid_members_and_empty_representations() {
        for (raw, expected) in [
            ("bytes=", HttpRangeError::Malformed),
            ("bytes=, ,", HttpRangeError::Malformed),
            ("bytes=0-1,broken", HttpRangeError::Malformed),
            ("bytes=9-5,0-1", HttpRangeError::Malformed),
            ("bytes=0-1,2-x", HttpRangeError::InvalidNumber),
            ("items=0-1", HttpRangeError::UnsupportedUnit),
        ] {
            assert_eq!(parse_byte_ranges(raw, 20, 8), Err(expected), "{raw}");
        }
        assert_eq!(
            parse_byte_ranges("bytes=-1,0-", 0, 2),
            Err(HttpRangeError::EmptyRepresentation)
        );
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
            ("bytes=9-5", HttpRangeError::Malformed),
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
