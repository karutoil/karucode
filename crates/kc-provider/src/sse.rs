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
        self.buffer.push_str(chunk);
        let mut out = Vec::new();
        while let Some(end) = find_event_end(&self.buffer) {
            let block: String = self.buffer.drain(..end).collect();
            if let Some(event) = parse_block(&block) {
                out.push(event);
            }
        }
        out
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
}
