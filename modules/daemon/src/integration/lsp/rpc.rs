//! JSON-RPC `Content-Length` framing for LSP stdio transport.

/// Encodes one framed message.
pub fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    let mut out = Vec::with_capacity(header.len() + payload.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(payload);
    out
}

/// The result of parsing the buffer head.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame<'a> {
    /// A full frame followed by the number of bytes it occupied.
    Complete {
        payload: &'a [u8],
        consumed: usize,
    },
    /// More bytes are needed before a frame can be extracted.
    Incomplete,
    /// The header block is malformed; the reader must skip `consumed` bytes
    /// (the whole header block) to resynchronize on the next frame.
    Malformed {
        consumed: usize,
    },
}

/// Parses at most one frame from the front of `buf`.
///
/// Returns [`Frame::Incomplete`] when neither a complete header nor the full
/// body announced by `Content-Length` is available yet, and
/// [`Frame::Malformed`] when a header terminator was seen but no usable
/// `Content-Length` could be extracted from it.
pub fn parse_frame(buf: &[u8]) -> Frame<'_> {
    const SEPARATOR: &[u8] = b"\r\n\r\n";
    let Some(offset) = find_subslice(buf, SEPARATOR) else {
        return Frame::Incomplete;
    };
    let skip_to = offset + SEPARATOR.len();
    let header = match std::str::from_utf8(&buf[..offset]) {
        Ok(text) => text,
        Err(_) => {
            return Frame::Malformed {
                consumed: skip_to,
            };
        }
    };
    let Some(length) = header.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("content-length") {
            return None;
        }
        value.trim().parse::<usize>().ok()
    }) else {
        return Frame::Malformed {
            consumed: skip_to,
        };
    };
    let body_start = skip_to;
    let Some(body_end) = body_start.checked_add(length) else {
        return Frame::Malformed {
            consumed: skip_to,
        };
    };
    if buf.len() < body_end {
        return Frame::Incomplete;
    }
    Frame::Complete {
        payload: &buf[body_start..body_start + length],
        consumed: body_start + length,
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(text: &str) -> Vec<u8> {
        encode_frame(text.as_bytes())
    }

    #[test]
    fn roundtrips_a_frame() {
        let bytes = frame(r#"{"jsonrpc":"2.0"}"#);
        match parse_frame(&bytes) {
            Frame::Complete {
                payload,
                consumed,
            } => {
                assert_eq!(payload, br#"{"jsonrpc":"2.0"}"#);
                assert_eq!(consumed, bytes.len());
            }
            Frame::Incomplete
            | Frame::Malformed {
                ..
            } => {
                panic!("expected a complete frame")
            }
        }
    }

    #[test]
    fn incomplete_body_reports_incomplete() {
        let mut bytes = frame("hello");
        bytes.truncate(bytes.len() - 2);
        assert_eq!(parse_frame(&bytes), Frame::Incomplete);
    }

    #[test]
    fn partial_header_reports_incomplete() {
        assert_eq!(parse_frame(b"Content-Len"), Frame::Incomplete);
    }

    #[test]
    fn parses_frames_split_across_reads() {
        let first = frame("one");
        let second = frame("two!");
        let mut combined = first.clone();
        combined.extend_from_slice(&second[..3]);

        let rest = match parse_frame(&combined) {
            Frame::Complete {
                consumed,
                ..
            } => &combined[consumed..],
            Frame::Incomplete
            | Frame::Malformed {
                ..
            } => panic!("first frame should parse"),
        };
        // Only part of the second frame arrived; reassemble and parse again.
        let mut tail = rest.to_vec();
        tail.extend_from_slice(&second[3..]);
        match parse_frame(&tail) {
            Frame::Complete {
                payload,
                consumed,
            } => {
                assert_eq!(payload, b"two!");
                assert_eq!(consumed, tail.len());
            }
            Frame::Incomplete
            | Frame::Malformed {
                ..
            } => {
                panic!("second frame should parse")
            }
        }
    }

    #[test]
    fn header_matching_is_case_insensitive() {
        let mut bytes = b"content-length: 2\r\n\r\nok".to_vec();
        match parse_frame(&bytes) {
            Frame::Complete {
                payload,
                ..
            } => assert_eq!(payload, b"ok"),
            Frame::Incomplete
            | Frame::Malformed {
                ..
            } => {
                panic!("expected a complete frame")
            }
        }
        bytes.clear();
        assert_eq!(parse_frame(&bytes), Frame::Incomplete);
    }

    #[test]
    fn malformed_header_is_skippable() {
        // A header terminator without a usable Content-Length must report the
        // header block as consumed so the reader can resynchronize.
        let mut bytes = b"garbage: 1\r\n\r\n".to_vec();
        bytes.extend_from_slice(&frame("ok"));
        match parse_frame(&bytes) {
            Frame::Malformed {
                consumed,
            } => {
                assert_eq!(consumed, b"garbage: 1\r\n\r\n".len());
                assert_eq!(parse_frame(&bytes[consumed..]), parse_frame(&frame("ok")));
            }
            _ => panic!("expected a malformed frame"),
        }
        // Non-UTF8 headers are equally recoverable.
        let binary = [0xffu8, b'\r', b'\n', b'\r', b'\n', b'x'];
        assert_eq!(
            parse_frame(&binary),
            Frame::Malformed {
                consumed: 5
            }
        );
    }
}
