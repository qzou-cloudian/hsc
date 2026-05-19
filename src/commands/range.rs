#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ByteRange {
    pub start: Option<u64>,
    pub len: Option<u64>,
}

#[derive(Clone, Copy)]
pub(crate) enum RangeErrorStyle {
    Cat,
    Cmp,
}

pub(crate) fn parse_range_options(
    range: Option<String>,
    offset: Option<u64>,
    size: Option<u64>,
    style: RangeErrorStyle,
) -> Result<ByteRange, Box<dyn std::error::Error>> {
    if let Some(range_str) = range {
        let range_part = range_str.strip_prefix("bytes=").unwrap_or(&range_str);
        let parts: Vec<&str> = range_part.split('-').collect();
        if parts.len() != 2 {
            return Err(invalid_format(&range_str, style).into());
        }

        let start = parts[0]
            .parse::<u64>()
            .map_err(|_| invalid_start(parts[0], style))?;

        let len = if parts[1].is_empty() {
            None
        } else {
            let end = parts[1]
                .parse::<u64>()
                .map_err(|_| invalid_end(parts[1], style))?;

            if end < start {
                return Err(invalid_order(style).into());
            }

            Some(end - start + 1)
        };

        Ok(ByteRange {
            start: Some(start),
            len,
        })
    } else if let Some(start) = offset {
        Ok(ByteRange {
            start: Some(start),
            len: size,
        })
    } else {
        Ok(ByteRange {
            start: None,
            len: None,
        })
    }
}

pub(crate) fn build_range_header(range: ByteRange) -> Option<String> {
    match (range.start, range.len) {
        (Some(start), Some(len)) => Some(format!("bytes={}-{}", start, start + len - 1)),
        (Some(start), None) => Some(format!("bytes={}-", start)),
        _ => None,
    }
}

pub(crate) fn bounded_len(range: ByteRange) -> Option<usize> {
    range.len.map(|len| len as usize)
}

fn invalid_format(range: &str, style: RangeErrorStyle) -> String {
    match style {
        RangeErrorStyle::Cat => format!(
            "Invalid range format: '{}'. Expected format: 'start-end' or 'start-'",
            range
        ),
        RangeErrorStyle::Cmp => format!("Invalid range '{}', expected 'start-end'", range),
    }
}

fn invalid_start(start: &str, style: RangeErrorStyle) -> String {
    match style {
        RangeErrorStyle::Cat => format!("Invalid start position in range: '{}'", start),
        RangeErrorStyle::Cmp => format!("Invalid range start '{}'", start),
    }
}

fn invalid_end(end: &str, style: RangeErrorStyle) -> String {
    match style {
        RangeErrorStyle::Cat => format!("Invalid end position in range: '{}'", end),
        RangeErrorStyle::Cmp => format!("Invalid range end '{}'", end),
    }
}

fn invalid_order(style: RangeErrorStyle) -> &'static str {
    match style {
        RangeErrorStyle::Cat => "End position must be greater than or equal to start position",
        RangeErrorStyle::Cmp => "Range end must be >= start",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_closed_range() {
        let range =
            parse_range_options(Some("0-9".to_string()), None, None, RangeErrorStyle::Cat).unwrap();
        assert_eq!(
            range,
            ByteRange {
                start: Some(0),
                len: Some(10)
            }
        );
        assert_eq!(build_range_header(range), Some("bytes=0-9".to_string()));
        assert_eq!(bounded_len(range), Some(10));
    }

    #[test]
    fn parses_prefixed_range() {
        let range = parse_range_options(
            Some("bytes=0-9".to_string()),
            None,
            None,
            RangeErrorStyle::Cat,
        )
        .unwrap();
        assert_eq!(
            range,
            ByteRange {
                start: Some(0),
                len: Some(10)
            }
        );
    }

    #[test]
    fn parses_open_ended_range() {
        let range = parse_range_options(Some("100-".to_string()), None, None, RangeErrorStyle::Cmp)
            .unwrap();
        assert_eq!(
            range,
            ByteRange {
                start: Some(100),
                len: None
            }
        );
        assert_eq!(build_range_header(range), Some("bytes=100-".to_string()));
        assert_eq!(bounded_len(range), None);
    }

    #[test]
    fn parses_offset_only() {
        let range = parse_range_options(None, Some(20), None, RangeErrorStyle::Cat).unwrap();
        assert_eq!(
            range,
            ByteRange {
                start: Some(20),
                len: None
            }
        );
    }

    #[test]
    fn parses_offset_and_size() {
        let range = parse_range_options(None, Some(20), Some(15), RangeErrorStyle::Cat).unwrap();
        assert_eq!(
            range,
            ByteRange {
                start: Some(20),
                len: Some(15)
            }
        );
        assert_eq!(build_range_header(range), Some("bytes=20-34".to_string()));
    }

    #[test]
    fn preserves_cat_malformed_error() {
        let err = parse_range_options(Some("bad".to_string()), None, None, RangeErrorStyle::Cat)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "Invalid range format: 'bad'. Expected format: 'start-end' or 'start-'"
        );
    }

    #[test]
    fn preserves_cmp_order_error() {
        let err = parse_range_options(Some("10-1".to_string()), None, None, RangeErrorStyle::Cmp)
            .unwrap_err()
            .to_string();
        assert_eq!(err, "Range end must be >= start");
    }
}
