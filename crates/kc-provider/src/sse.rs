//! Minimal SSE line parser used by the provider and MCP HTTP transport.

#[derive(Default)]
pub struct SseParser {
    buffer: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes; returns every complete event found so far.
    pub fn push(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.push_limited(chunk, usize::MAX).0
    }

    /// Parse complete blocks up to `max_bytes` raw bytes each, including the
    /// terminator. Return already parsed events and whether a block exceeded it.
    pub fn push_limited(&mut self, chunk: &str, max_bytes: usize) -> (Vec<SseEvent>, bool) {
        self.buffer.push_str(chunk);
        // A proxy may prefix the stream with a UTF-8 BOM. Left in place it turns
        // the first `data:` line into an unrecognised one and silently drops the
        // first event, so strip it once at the very start of the stream.
        if self.buffer.starts_with('\u{FEFF}') {
            self.buffer.drain(..'\u{FEFF}'.len_utf8());
        }
        let mut out = Vec::new();
        while let Some(end) = find_event_end(&self.buffer) {
            if end > max_bytes {
                self.buffer.clear();
                return (out, true);
            }
            let block: String = self.buffer.drain(..end).collect();
            if let Some(event) = parse_block(&block) {
                out.push(event);
            }
        }
        let oversized = self.buffer.len() > max_bytes;
        if oversized {
            self.buffer.clear();
        }
        (out, oversized)
    }

    /// Bytes held back because no event terminator has been seen yet. Callers
    /// use this to bound memory when an upstream never terminates an event.
    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    /// Flush a trailing event that was not terminated by a blank line before the
    /// stream ended. Servers commonly omit the final `\n\n`.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if self.buffer.trim().is_empty() {
            self.buffer.clear();
            return None;
        }
        let block = std::mem::take(&mut self.buffer);
        parse_block(&block)
    }
}

/// Length of the buffer up to and including the first event terminator. Both
/// `\n\n` and `\r\n\r\n` are accepted; the latter contains no `\n\n`.
fn find_event_end(buffer: &str) -> Option<usize> {
    let lf = buffer.find("\n\n").map(|idx| idx + 2);
    let crlf = buffer.find("\r\n\r\n").map(|idx| idx + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn parse_block(block: &str) -> Option<SseEvent> {
    let mut event = None;
    let mut data = String::new();
    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
    }
    if data.is_empty() {
        return None;
    }
    Some(SseEvent { event, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_blocks_are_checked_before_parsing() {
        for terminator in ["\n\n", "\r\n\r\n"] {
            let exact = format!("data: x{terminator}");
            let limit = exact.len();
            let mut p = SseParser::new();
            let (events, oversized) = p.push_limited(&exact, limit);
            assert!(!oversized);
            assert_eq!(events[0].data, "x");

            let mut p = SseParser::new();
            let input = format!("data: a\n\ndata: xx{terminator}data: b\n\n");
            let (events, oversized) = p.push_limited(&input, limit);
            assert!(oversized);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].data, "a");

            let mut p = SseParser::new();
            let split = format!("data: xx{terminator}");
            let (events, oversized) = p.push_limited(&split[..limit], limit);
            assert!(events.is_empty());
            assert!(!oversized);
            assert!(p.push_limited(&split[limit..], limit).1);
        }
        let mut p = SseParser::new();
        assert!(p.push_limited(": oversized comment\n\n", 8).1);
        let mut p = SseParser::new();
        assert!(p.push_limited("data: unfinished", 8).1);
    }

    #[test]
    fn parses_split_events() {
        let mut p = SseParser::new();
        assert!(p.push("data: {\"a\":").is_empty());
        let evs = p.push("1}\n\ndata: [DONE]\n\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].data, "{\"a\":1}");
        assert_eq!(evs[1].data, "[DONE]");
    }

    #[test]
    fn parses_multiline_data_and_event_name() {
        let mut p = SseParser::new();
        let evs = p.push("event: message\r\ndata: a\r\ndata: b\r\n\r\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event.as_deref(), Some("message"));
        assert_eq!(evs[0].data, "a\nb");
    }

    #[test]
    fn finish_returns_unterminated_tail() {
        let mut p = SseParser::new();
        assert!(p.push("data: [DONE]").is_empty());
        assert_eq!(p.buffered_len(), 12);
        assert_eq!(p.finish().unwrap().data, "[DONE]");
        assert_eq!(p.buffered_len(), 0);
        assert!(p.finish().is_none());
    }

    #[test]
    fn comment_only_blocks_are_ignored() {
        let mut p = SseParser::new();
        assert!(p.push(": keepalive\n\n").is_empty());
        assert!(p.buffered_len() == 0);
    }

    /// Keep-alive comments and blank lines arrive between real events on every
    /// long-lived gateway stream; they must not swallow or merge the events.
    #[test]
    fn keepalives_between_events_are_transparent() {
        let mut p = SseParser::new();
        let evs = p.push(": ping\n\ndata: a\n\n: ping\n\ndata: b\n\n");
        assert_eq!(evs.len(), 2, "{evs:?}");
        assert_eq!(evs[0].data, "a");
        assert_eq!(evs[1].data, "b");
        assert_eq!(p.buffered_len(), 0);
    }

    /// A comment line *inside* an event block must not corrupt its data.
    #[test]
    fn comment_inside_an_event_is_ignored() {
        let mut p = SseParser::new();
        let evs = p.push("data: a\n: note\ndata: b\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "a\nb");
    }

    /// CRLF terminators split across chunk boundaries: the `\r\n\r\n` sequence
    /// only becomes visible once the second chunk arrives.
    #[test]
    fn crlf_split_across_chunk_boundaries() {
        let mut p = SseParser::new();
        assert!(p.push("data: a\r").is_empty());
        assert!(p.push("\n\r").is_empty());
        let evs = p.push("\ndata: b\r\n\r\n");
        assert_eq!(evs.len(), 2, "{evs:?}");
        assert_eq!(evs[0].data, "a");
        assert_eq!(evs[1].data, "b");
    }

    /// A JSON object split at an arbitrary byte boundary must be reassembled.
    #[test]
    fn json_split_across_chunk_boundaries() {
        let mut p = SseParser::new();
        assert!(p.push("data: {\"choices\":[{\"delta\":{\"con").is_empty());
        assert!(p.push("tent\":\"hi\"}}]}\n").is_empty());
        let evs = p.push("\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(
            evs[0].data,
            "{\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}"
        );
    }

    /// A UTF-8 byte order mark is a real proxy artifact; it must not turn the
    /// first event into an unparseable line.
    #[test]
    fn byte_order_mark_is_tolerated() {
        let mut p = SseParser::new();
        let evs = p.push("\u{FEFF}data: a\n\ndata: b\n\n");
        assert_eq!(evs.len(), 2, "{evs:?}");
        assert_eq!(evs[0].data, "a");
        assert_eq!(evs[1].data, "b");
    }

    /// `data:` with no space after the colon is valid SSE.
    #[test]
    fn data_without_a_space_is_parsed() {
        let mut p = SseParser::new();
        let evs = p.push("data:{\"a\":1}\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "{\"a\":1}");
    }
}
