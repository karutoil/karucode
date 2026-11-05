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
        // Parse by offset and move the remaining bytes once at the end. Draining
        // each event from the front made a chunk containing N events move the
        // remainder N times (quadratic in the chunk size).
        let mut consumed = 0;
        while let Some(end) = find_event_end(&self.buffer, consumed) {
            if end > max_bytes {
                self.buffer.clear();
                return (out, true);
            }
            if let Some(event) = parse_block(&self.buffer[consumed..end]) {
                out.push(event);
            }
            consumed = end;
        }
        if consumed > 0 {
            self.buffer.drain(..consumed);
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

/// Length of the buffer up to and including the first event terminator at or
/// after `from`. Keep the original LF and CRLF delimiter contract; the offset is
/// what avoids rescanning already-consumed events and front-draining each block.
fn find_event_end(buffer: &str, from: usize) -> Option<usize> {
    let bytes = buffer.as_bytes();
    for at in from..bytes.len() {
        if bytes[at..].starts_with(b"\n\n") {
            return Some(at + 2);
        }
        if bytes[at..].starts_with(b"\r\n\r\n") {
            return Some(at + 4);
        }
    }
    None
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
    fn many_events_in_one_chunk_drain_linearly() {
        const EVENTS: usize = 20_000;
        let payload = "x".repeat(128);
        let mut input = String::with_capacity(EVENTS * (payload.len() + 8));
        for _ in 0..EVENTS {
            input.push_str("data: ");
            input.push_str(&payload);
            input.push_str("\n\n");
        }
        let started = std::time::Instant::now();
        let mut parser = SseParser::new();
        let events = parser.push(&input);
        assert_eq!(events.len(), EVENTS);
        assert_eq!(parser.buffered_len(), 0);
        assert!(events.iter().all(|event| event.data == payload));
        // 20k events used to front-drain a multi-megabyte buffer 20k times. This
        // is a regression guard, not a benchmark with a machine-tight threshold.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "parsing {EVENTS} events took {:?}",
            started.elapsed()
        );
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
