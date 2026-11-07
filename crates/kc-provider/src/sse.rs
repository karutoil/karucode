//! Minimal SSE line parser used by the provider and MCP HTTP transport.

#[derive(Default)]
pub struct SseParser {
    buffer: String,
    /// Where the next search for an event delimiter may safely resume. An
    /// unterminated event used to be searched from byte zero after every network
    /// read, which made an incomplete event quadratic to receive.
    scan_from: usize,
    /// Any non-empty body chunk arrived, including comments or non-SSE text.
    saw_input: bool,
    /// A trailing CR removed until we know whether the next chunk starts with
    /// LF. Keeping this outside `buffer` prevents a trailing CR from becoming a
    /// lone CR first and then a new CRLF when the network splits the pair.
    pending_cr: bool,
    /// An event delimiter ended in CR at the old end of input. If the next chunk
    /// starts with LF, it only completes that delimiter's CRLF and must not start
    /// another event.
    discard_lf: bool,
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
        self.saw_input |= !chunk.is_empty();
        let chunk = if std::mem::take(&mut self.discard_lf) && chunk.starts_with('\n') {
            &chunk[1..]
        } else {
            chunk
        };
        if std::mem::take(&mut self.pending_cr) {
            // Restore the held byte before the new bytes. A leading LF now forms
            // one CRLF; any other byte makes it a lone CR. Neither duplicate nor
            // drop the LF.
            self.buffer.push('\r');
        }
        self.buffer.push_str(chunk);
        // A proxy may prefix the stream with a UTF-8 BOM. Left in place it turns
        // the first `data:` line into an unrecognised one and silently drops the
        // first event, so strip it once at the very start of the stream.
        if self.buffer.starts_with('\u{FEFF}') {
            self.buffer.drain(..'\u{FEFF}'.len_utf8());
            self.scan_from = 0;
        }
        let mut out = Vec::new();
        // Parse by offset and move the remaining bytes once at the end. Draining
        // each event from the front made a chunk containing N events move the
        // remainder N times (quadratic in the chunk size). The persisted cursor
        // also keeps an unterminated final event from being searched afresh on
        // every push; at most three bytes are re-read for a split delimiter.
        let mut consumed = 0;
        loop {
            let from = if consumed == 0 {
                self.scan_from.min(self.buffer.len())
            } else {
                consumed
            };
            let Some(end) = find_event_end(&self.buffer, from) else {
                break;
            };
            // `end` is a buffer offset, not this event's length. Earlier events
            // in the same push must not count against this event's limit.
            if end - consumed > max_bytes {
                self.buffer.clear();
                self.scan_from = 0;
                self.pending_cr = false;
                self.discard_lf = false;
                return (out, true);
            }
            if let Some(event) = parse_block(&self.buffer[consumed..end]) {
                out.push(event);
            }
            if end == self.buffer.len() && self.buffer.as_bytes().last() == Some(&b'\r') {
                self.discard_lf = true;
            }
            consumed = end;
        }
        if consumed > 0 {
            self.buffer.drain(..consumed);
        }
        // Defer one trailing CR until the next chunk decides between lone CR and
        // CRLF. The longest complete delimiter is CRLF+CRLF (four bytes), so only
        // the last three retained bytes are ever re-read on a later push.
        if self.buffer.ends_with('\r') {
            self.buffer.pop();
            self.pending_cr = true;
        }
        self.scan_from = self.buffer.len().saturating_sub(3);
        let oversized = self.buffered_len() > max_bytes;
        if oversized {
            self.buffer.clear();
            self.scan_from = 0;
            self.pending_cr = false;
            self.discard_lf = false;
        }
        (out, oversized)
    }

    /// Bytes held back because no event terminator has been seen yet. Callers
    /// use this to bound memory when an upstream never terminates an event.
    pub fn buffered_len(&self) -> usize {
        self.buffer.len() + usize::from(self.pending_cr)
    }

    /// Flush a trailing event that was not terminated by a blank line before the
    /// stream ended. Servers commonly omit the final `\n\n`.
    pub fn finish(&mut self) -> Option<SseEvent> {
        self.scan_from = 0;
        self.discard_lf = false;
        if std::mem::take(&mut self.pending_cr) {
            self.buffer.push('\r');
        }
        if self.buffer.trim().is_empty() {
            self.buffer.clear();
            return None;
        }
        let block = std::mem::take(&mut self.buffer);
        parse_block(&block)
    }

    /// Whether any body bytes arrived. EOF with none is an empty answer, not a
    /// keepalive-only or non-SSE body that another attempt may repair.
    pub(crate) fn has_input(&self) -> bool {
        self.saw_input
    }

    /// Record raw body bytes even when an incomplete UTF-8 sequence means the
    /// decoder has no text to hand to the parser yet.
    pub(crate) fn note_input(&mut self) {
        self.saw_input = true;
    }

    #[cfg(test)]
    fn resume_offset(&self) -> usize {
        self.scan_from
    }
}

/// Length of the buffer up to and including the first event terminator at or
/// after `from`. SSE permits LF, CRLF and lone CR line endings in any mixture;
/// an event ends when two line endings are adjacent (a blank line).
fn find_event_end(buffer: &str, from: usize) -> Option<usize> {
    let bytes = buffer.as_bytes();
    let mut at = from;
    while at < bytes.len() {
        let Some(first) = line_ending_len(bytes, at) else {
            at += 1;
            continue;
        };
        let second_at = at + first;
        let second = line_ending_len(bytes, second_at).or_else(|| {
            // Two endings are already a dispatchable blank line. Whether this
            // final CR later grows into CRLF only changes the continuation LF,
            // which `discard_lf` consumes instead of treating it as new input.
            (second_at + 1 == bytes.len() && bytes[second_at] == b'\r').then_some(1)
        });
        if let Some(second) = second {
            return Some(second_at + second);
        }
        at += first;
    }
    None
}

/// Length of one SSE line ending at `at`. A trailing CR is deliberately not
/// decided until the next chunk: it may be the first half of CRLF.
fn line_ending_len(bytes: &[u8], at: usize) -> Option<usize> {
    match bytes.get(at).copied() {
        Some(b'\n') => Some(1),
        Some(b'\r') if at + 1 < bytes.len() => Some(if bytes[at + 1] == b'\n' { 2 } else { 1 }),
        _ => None,
    }
}

fn split_lines(block: &str) -> impl Iterator<Item = &str> {
    // CRLF produces an empty item between its two delimiters. It is not a data
    // field and is ignored, while lone CR and mixed endings now behave like LF.
    block.split(['\n', '\r'])
}

fn parse_block(block: &str) -> Option<SseEvent> {
    let mut event = None;
    let mut data = String::new();
    for line in split_lines(block) {
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
        for terminator in ["\n\n", "\r\n\r\n", "\r\r", "\n\r", "\n\r\n", "\r\n\n"] {
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
            // Stop before the trailing CR as well as its optional LF: LF+CR is
            // already a valid mixed blank line, so cutting between CR and LF can
            // validly dispatch without exceeding the limit.
            let split_at = limit - 1;
            let (events, oversized) = p.push_limited(&split[..split_at], limit);
            assert!(events.is_empty(), "terminator {terminator:?}: {events:?}");
            assert!(!oversized);
            assert!(p.push_limited(&split[split_at..], limit).1);
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
        let mut evs = p.push("data: a\r");
        assert!(evs.is_empty());
        // CRLF plus a trailing CR already makes a blank line. A subsequent LF is
        // only that CR's continuation and must not create an empty third event.
        evs.extend(p.push("\n\r"));
        evs.extend(p.push("\ndata: b\r\n\r\n"));
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

    #[test]
    fn cr_only_and_mixed_blank_lines_dispatch_at_every_split() {
        let body = "data: a\r\rdata: b\ndata: b2\n\rdata: c\r\n\ndata: d\r\n\n";
        for split in 0..=body.len() {
            let mut p = SseParser::new();
            let mut events = p.push(&body[..split]);
            events.extend(p.push(&body[split..]));
            let data: Vec<&str> = events.iter().map(|event| event.data.as_str()).collect();
            assert_eq!(data, ["a", "b\nb2", "c", "d"], "split at {split}");
        }

        let mut p = SseParser::new();
        let mut data = Vec::new();
        for byte in body.as_bytes() {
            let ch = char::from(*byte).to_string();
            data.extend(p.push(&ch).into_iter().map(|event| event.data));
        }
        assert_eq!(data, ["a", "b\nb2", "c", "d"]);

        let mut p = SseParser::new();
        assert_eq!(p.push("data: after-cr\n\r")[0].data, "after-cr");
        // The LF only completes the already-emitted delimiter's CRLF; it does not
        // add a line to or merge with the following event.
        assert!(p.push("\n").is_empty());
        assert_eq!(p.push("data: next\r\r")[0].data, "next");
    }

    #[test]
    fn an_unterminated_event_is_scanned_incrementally() {
        const TOTAL: usize = 512 * 1024;
        const CHUNK: usize = 128;
        let filler = "x".repeat(CHUNK);
        let mut parser = SseParser::new();
        assert!(parser.push("data: ").is_empty());
        let started = std::time::Instant::now();
        for _ in 0..(TOTAL / CHUNK) {
            assert!(parser.push(&filler).is_empty());
            assert!(
                parser.resume_offset().saturating_add(3) >= parser.buffered_len(),
                "the cursor must not fall back to the start of an open event"
            );
        }
        assert_eq!(parser.buffered_len(), TOTAL + 6);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "512 KiB in 128-byte reads took {:?}; incomplete events must not be rescanned",
            started.elapsed()
        );
        assert_eq!(parser.finish().map(|event| event.data.len()), Some(TOTAL));
    }
}
