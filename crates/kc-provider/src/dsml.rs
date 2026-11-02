//! Recovery of DeepSeek tool calls that arrive as **text** instead of in
//! `tool_calls`.
//!
//! DeepSeek-V4 models are trained on their own markup language (DSML) and
//! sometimes write it into the ordinary `content` channel:
//!
//! ```text
//! I will look at the file first.<||DSML||tool_calls><||DSML||invoke name="read_file"><||DSML||parameter name="path" string="true">src/main.rs</||DSML||parameter></||DSML||invoke></||DSML||tool_calls>
//! ```
//!
//! A server that understands DSML turns that into structured `tool_calls`. An
//! OpenAI-compatible endpoint that does not hands it back as prose, so the agent
//! prints the markup as an answer and never runs the tool: the model looks like
//! it stopped working. That is the failure this module fixes, and it is the same
//! one upstream reports against the model
//! (<https://huggingface.co/deepseek-ai/DeepSeek-V4-Pro/discussions/209>).
//!
//! Three properties matter more than completeness:
//!
//! * **Nothing is executed unless the model named a real tool.** The request's
//!   tool names are the authority: a block whose invokes do not all name a tool
//!   this run could call is left in the transcript exactly as it arrived. A
//!   model explaining the format ("write your call as `<||DSML||tool_calls>`")
//!   must not become a tool call, and nothing is guessed from the model id.
//! * **Streaming survives.** The markers are long, so the network splits them at
//!   arbitrary points. Text that may still be the start of a marker is held back
//!   until it is known not to be one, which is what stops a half-arrived
//!   `<||DSML||` from being rendered as chat and then vanishing.
//! * **Text is never lost silently.** Anything that is not turned into a call is
//!   flushed back to the transcript verbatim, including a block that never
//!   closed because the model was cut off mid-call.
//!
//! The grammar is the one the model was trained on (`legend/deepseek-v4-vllm-patches`,
//! `vllm/parser/deepseek_v4.py`), with deliberately wider tolerance because a
//! gateway in between may mangle the markers: the full-width `｜DSML｜` and the
//! degraded `||DSML||` spelling are both accepted, whitespace around the bars and
//! inside the tags is ignored, `tool_calls`/`toolcalls`/`tool` all open a block,
//! and a `parameter` without a `string` attribute is decoded as JSON with a
//! literal fallback.

use std::collections::HashSet;

/// Full-width spelling of the DSML namespace token, as the model was trained.
const DSML: &str = "DSML";

/// A recovered call, ready to be handed to the runtime as a `ToolCallDelta`.
#[derive(Debug, PartialEq)]
pub(crate) struct DsmlCall {
    pub(crate) name: String,
    /// A JSON object, as a string - the same shape the native `tool_calls` path
    /// produces, so nothing downstream can tell the difference.
    pub(crate) arguments: String,
}

/// What is safe to emit now.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Step {
    pub(crate) text: String,
    pub(crate) calls: Vec<DsmlCall>,
}

/// The tool names this request could actually call.
#[derive(Debug)]
pub(crate) struct DsmlTools {
    names: HashSet<String>,
}

impl DsmlTools {
    /// `None` when the request has no tools: a DSML block in a turn that could
    /// not call anything is a quotation, not a call.
    pub(crate) fn new(names: impl IntoIterator<Item = String>) -> Option<Self> {
        let names: HashSet<String> = names.into_iter().collect();
        if names.is_empty() {
            None
        } else {
            Some(Self { names })
        }
    }

    fn knows(&self, name: &str) -> bool {
        self.names.contains(name)
    }
}

/// Incremental `content` -> (text, calls) filter.
///
/// Two states only: outside a block, and inside one. The nested `invoke` /
/// `parameter` markers are the block parser's problem, not the streaming state
/// machine's, which is why a block is buffered verbatim until it closes.
pub(crate) struct DsmlScanner {
    tools: DsmlTools,
    /// Text that has not been consumed yet.
    buf: String,
    /// An open block: offset of its start marker inside `buf`.
    block: Option<usize>,
    /// How far the search for this block's closing marker has already got.
    ///
    /// An open block is the one case where `buf` grows without bound, so
    /// re-walking it from the top on every delta is quadratic: measured 22 s for
    /// one megabyte of unclosed block, against a `MAX_BLOCK_BYTES` ten times
    /// that. [`find_marker_from`] reports where a failed search may resume, so
    /// each delta only scans the bytes that arrived since the last one.
    scanned: usize,
}

/// A block longer than this is not a tool call any more. Give up on it and let
/// the text through rather than buffering without bound.
const MAX_BLOCK_BYTES: usize = 1024 * 1024;

impl DsmlScanner {
    pub(crate) fn new(tools: DsmlTools) -> Self {
        Self {
            tools,
            buf: String::new(),
            block: None,
            scanned: 0,
        }
    }

    /// Feed one `content` delta.
    pub(crate) fn push(&mut self, delta: &str) -> Step {
        self.buf.push_str(delta);
        let mut step = Step::default();
        loop {
            match self.block {
                None => {
                    let Some(found) = find_marker(&self.buf, 0, Tag::ToolCalls, false) else {
                        // Hold back a tail that could still become a marker.
                        let keep = partial_tail(&self.buf);
                        let cut = self.buf.len() - keep;
                        step.text.push_str(&self.buf[..cut]);
                        self.buf.drain(..cut);
                        return step;
                    };
                    step.text.push_str(&self.buf[..found.at]);
                    self.buf.drain(..found.at);
                    self.block = Some(0);
                    self.scanned = 0;
                }
                Some(start) => {
                    let from = close_hint(&self.buf, start).max(self.scanned);
                    let (found, resume) = find_marker_from(&self.buf, from, Tag::ToolCalls, true);
                    let Some(found) = found else {
                        // Everything before `resume` can never become the start of
                        // a closing marker, so the next delta starts looking
                        // there instead of at the front of an ever-growing block.
                        self.scanned = resume;
                        if self.buf.len() > MAX_BLOCK_BYTES {
                            tracing::warn!(
                                bytes = self.buf.len(),
                                "abandoning a DSML tool-call block that grew past {MAX_BLOCK_BYTES} bytes"
                            );
                            step.text.push_str(&std::mem::take(&mut self.buf));
                            self.block = None;
                            self.scanned = 0;
                        }
                        return step;
                    };
                    // Only the block leaves the buffer; what the model wrote
                    // after it stays for the next round.
                    let block = self.buf[..found.end].to_string();
                    self.buf.drain(..found.end);
                    self.block = None;
                    self.scanned = 0;
                    step.extend(self.decide(&block, start, found.end));
                }
            }
        }
    }

    /// End of message. Anything still held is text: an unterminated block is a
    /// model that was cut off, not a call.
    pub(crate) fn finish(&mut self) -> Step {
        let mut step = Step::default();
        step.text.push_str(&std::mem::take(&mut self.buf));
        self.block = None;
        self.scanned = 0;
        step
    }

    /// A block closed. Either it is a real call, or its markup returns to the
    /// transcript exactly as the model wrote it.
    fn decide(&self, block: &str, start: usize, end: usize) -> Step {
        let mut step = Step::default();
        // Prose the model wrote between an earlier marker and this block's opener
        // (`start` is non-zero when a stray closing marker interrupted it).
        step.text.push_str(&block[..start]);
        let markup = &block[start..end];
        match parse_block(markup, &self.tools) {
            Parsed::Calls(calls) => step.calls = calls,
            Parsed::Unrecognised { names } => {
                tracing::warn!(
                    tools = %names.join(", "),
                    "the model emitted DSML tool-call markup naming a tool this run does not \
                     have; leaving it in the transcript as text"
                );
                step.text.push_str(markup);
            }
            Parsed::NotACall => {
                tracing::debug!("DSML markup did not parse into a tool call; keeping it as text");
                step.text.push_str(markup);
            }
        }
        step
    }
}

impl Step {
    fn extend(&mut self, other: Step) {
        self.text.push_str(&other.text);
        self.calls.extend(other.calls);
    }
}

/// Where to resume the search for a block's closing marker: never before the
/// opening marker, which contains a similar tag name.
///
/// The opening marker is always complete - `find_marker` only returns one whose
/// `>` has arrived - so this is a single scan of the marker at `start`, not a
/// walk of the whole block. (Re-walking the block from offset 0 here is what
/// made an unclosed block quadratic; the resume point of the failed search is
/// kept in [`DsmlScanner::scanned`], so the search never starts over.)
fn close_hint(buf: &str, start: usize) -> usize {
    match find_marker(buf, start, Tag::ToolCalls, false) {
        Some(found) => found.end,
        None => start,
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Tag {
    ToolCalls,
    Invoke,
    Parameter,
}

/// A marker recognised in the text.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Found {
    /// Byte offset of the `<`.
    at: usize,
    /// Byte offset just past the tag name.
    end: usize,
    /// A `</...>` marker.
    close: bool,
    tag: Tag,
}

/// Find the next marker of `tag` at or after `from`. With `close = true` only a
/// closing marker matches, with `close = false` only an opening one.
fn find_marker(text: &str, from: usize, tag: Tag, close: bool) -> Option<Found> {
    find_marker_from(text, from, tag, close).0
}

/// [`find_marker`] that also reports where a search which found nothing may be
/// resumed from once more text arrives.
///
/// The result of a failed search is `(None, resume)`: `resume <= text.len()`, and
/// appending to `text` cannot change what `find_marker` returns for any `from`
/// in `0..resume`. Byte comparisons are translation invariant, so appending is
/// the only way a later search for the same tag and polarity can differ - which
/// is what makes this a safe cursor for the streaming scanner's open block,
/// where the buffer grows by one delta at a time and rescanning it from the
/// front would be quadratic.
///
/// On a match the second element is `found.at`, i.e. where a search for another
/// marker could carry on.
fn find_marker_from(text: &str, from: usize, tag: Tag, close: bool) -> (Option<Found>, usize) {
    let mut at = from;
    while let Some(found) = text[at..].find('<').map(|pos| at + pos) {
        match scan_marker(text, found) {
            Scan::Match(marker) if marker.tag == tag && marker.close == close => {
                return (Some(marker), found);
            }
            // A complete marker: scanning consumed it whole, so no `<` can hide
            // between `found` and its end and the search can skip to there.
            Scan::Match(marker) => at = marker.end,
            // The bytes from here to the next `<` are plain text. They cannot
            // become a marker whatever arrives next.
            Scan::Mismatch => at = found + 1,
            // Ran out of input mid-marker: the rest may still arrive, so the
            // search resumes from the `<` that started it.
            Scan::Incomplete => return (None, found),
        }
    }
    // No `<` is left, so the next candidate can only be in text that has not
    // arrived yet: everything already here is safe to skip.
    (None, text.len())
}

enum Scan {
    Match(Found),
    /// The text ended inside something that is still a plausible marker.
    Incomplete,
    Mismatch,
}

fn scan_marker(text: &str, at: usize) -> Scan {
    let mut cur = Cursor::new(text, at);
    if !cur.eat(b'<') {
        return Scan::Mismatch;
    }
    let close = cur.eat(b'/');
    // One "bar group" on each side of the namespace token: the full-width `｜`
    // the model was trained on, or the degraded `||` (or a single `|`) that some
    // gateways and quantised models emit instead. Whitespace anywhere inside the
    // tag is ignored, because a model that lost the exact spacing still meant to
    // make a call.
    cur.skip_ws();
    if !cur.eat_bar_run() {
        return cur.incomplete_or_mismatch();
    }
    cur.skip_ws();
    match cur.eat_ci(DSML) {
        Eat::Done => {}
        Eat::Incomplete => return Scan::Incomplete,
        Eat::Mismatch => return Scan::Mismatch,
    }
    cur.skip_ws();
    if !cur.eat_bar_run() {
        return cur.incomplete_or_mismatch();
    }
    cur.skip_ws();
    let Some(name) = cur.eat_name() else {
        return cur.incomplete_or_mismatch();
    };
    // A tag name cut off by the end of the buffer is not "some other tag": it is
    // a marker still arriving, and holding it back is what stops half of
    // `<|DSML|tool_calls` from being rendered as chat.
    if cur.at_end() && is_prefix_of_tag(&name) {
        return Scan::Incomplete;
    }
    let tag = if name.eq_ignore_ascii_case("tool_calls")
        || name.eq_ignore_ascii_case("toolcalls")
        // `vllm/parser/deepseek_v4.py` records this spelling from production.
        || name.eq_ignore_ascii_case("tool")
    {
        Tag::ToolCalls
    } else if name.eq_ignore_ascii_case("invoke") {
        Tag::Invoke
    } else if name.eq_ignore_ascii_case("parameter") {
        Tag::Parameter
    } else {
        return Scan::Mismatch;
    };
    // Consume the tag's `>` when it is already there, so `end` means "past this
    // tag" for the block delimiters. An `invoke`/`parameter` carries attributes
    // between the name and its `>`, which their parser reads for itself.
    cur.skip_ws();
    // A marker is not complete until its `>` arrives. Without this,
    // `<|DSML|tool_calls` is matched as a whole opener the moment the tag name
    // is buffered, and the `>` that follows is then re-read as body text.
    if !cur.text[cur.pos..].contains(&b'>') {
        return Scan::Incomplete;
    }
    cur.eat(b'>');
    Scan::Match(Found {
        at,
        end: cur.pos,
        close,
        tag,
    })
}

/// Tag names the scanner knows, for spotting one that is still arriving.
const TAG_NAMES: [&str; 6] = [
    "tool_calls",
    "toolcalls",
    "tool",
    "invoke",
    "parameter",
    "think",
];

fn is_prefix_of_tag(name: &str) -> bool {
    TAG_NAMES
        .iter()
        .any(|tag| tag.len() > name.len() && tag[..name.len()].eq_ignore_ascii_case(name))
}

/// A byte cursor that knows how far it got, so a marker split by the network can
/// be told apart from text that was never a marker.
struct Cursor<'a> {
    text: &'a [u8],
    pos: usize,
    /// The last thing that stopped the scan.
    ran_out: bool,
}

enum Eat {
    Done,
    Incomplete,
    Mismatch,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str, at: usize) -> Self {
        Self {
            text: text.as_bytes(),
            pos: at,
            ran_out: false,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.pos).copied()
    }

    fn at_end(&self) -> bool {
        self.pos >= self.text.len()
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.pos += 1;
            true
        } else {
            self.ran_out = self.peek().is_none();
            false
        }
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    /// The namespace token is wrapped in a run of bars on each side: the
    /// full-width `｜` the model was trained on, or the degraded `||` (or a
    /// lone `|`) that quantised models and rewriting gateways emit. Accept
    /// either, one or two of them, with whitespace in between.
    fn eat_bar_run(&mut self) -> bool {
        let mut count = 0;
        loop {
            let before = self.pos;
            self.skip_ws();
            if !self.eat_bar() {
                // Put the whitespace back: it may belong to whatever follows.
                self.pos = before;
                break;
            }
            count += 1;
            if count == 2 {
                break;
            }
        }
        count > 0
    }

    /// One bar: the full-width `｜` (`\u{ff5c}`, three bytes) or ASCII `|`.
    fn eat_bar(&mut self) -> bool {
        if self.text[self.pos..].starts_with("\u{ff5c}".as_bytes()) {
            self.pos += "\u{ff5c}".len();
            return true;
        }
        self.eat(b'|')
    }

    fn eat_ci(&mut self, expected: &str) -> Eat {
        for byte in expected.bytes() {
            match self.peek() {
                None => return Eat::Incomplete,
                Some(found) if found.eq_ignore_ascii_case(&byte) => self.pos += 1,
                Some(_) => return Eat::Mismatch,
            }
        }
        Eat::Done
    }

    /// A tag name: ASCII alphanumerics, `_` and `-`. Reading the whole run (not
    /// stopping at a match) is what keeps `<|DSML|toolbox>` from matching `tool`.
    fn eat_name(&mut self) -> Option<String> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        std::str::from_utf8(&self.text[start..self.pos])
            .ok()
            .map(str::to_string)
    }

    fn incomplete_or_mismatch(&self) -> Scan {
        // Stopping *at* the end of the buffer is never a mismatch: more input
        // may still complete the marker. `ran_out` covers the case where a
        // specific byte was missing.
        if self.ran_out || self.at_end() {
            Scan::Incomplete
        } else {
            Scan::Mismatch
        }
    }
}

/// Bytes at the end of `text` that could still grow into a marker.
///
/// Only the tail from the last `<` matters: everything before it is ordinary
/// text that can be shown immediately.
fn partial_tail(text: &str) -> usize {
    let Some(start) = text.rfind('<') else {
        return 0;
    };
    match scan_marker(text, start) {
        Scan::Incomplete => text.len() - start,
        _ => 0,
    }
}

/// The result of reading one complete block.
enum Parsed {
    Calls(Vec<DsmlCall>),
    /// Invokes named tools that are not available to this run.
    Unrecognised {
        names: Vec<String>,
    },
    NotACall,
}

/// One decoded `invoke`.
struct Invoke {
    name: String,
    /// `(name, type attribute, raw value)`.
    parameters: Vec<(String, Option<String>, String)>,
}

/// Parse a block that starts at its opening marker and ends after its closing
/// one.
fn parse_block(markup: &str, tools: &DsmlTools) -> Parsed {
    let Some(open) = find_marker(markup, 0, Tag::ToolCalls, false) else {
        return Parsed::NotACall;
    };
    let Some(close) = find_marker(markup, open.end, Tag::ToolCalls, true) else {
        return Parsed::NotACall;
    };
    let body = &markup[open.end..close.at];

    let invokes = parse_invokes(body);
    if invokes.is_empty() {
        return Parsed::NotACall;
    }
    // Every invoke must name a tool this run could call. One unknown name makes
    // the whole block a quotation: executing the known half of a block the model
    // half-invented is worse than showing the operator what it asked for.
    let unknown: Vec<String> = invokes
        .iter()
        .filter(|invoke| !tools.knows(&invoke.name))
        .map(|invoke| invoke.name.clone())
        .collect();
    if !unknown.is_empty() {
        return Parsed::Unrecognised { names: unknown };
    }
    Parsed::Calls(
        invokes
            .into_iter()
            .map(|invoke| DsmlCall {
                name: invoke.name,
                arguments: arguments_json(&invoke.parameters),
            })
            .collect(),
    )
}

fn parse_invokes(body: &str) -> Vec<Invoke> {
    let mut invokes = Vec::new();
    let mut from = 0;
    while let Some(open) = find_marker(body, from, Tag::Invoke, false) {
        // `open.end` is past the tag name; its attributes and `>` come next.
        let header_end = match body[open.end..].find('>') {
            Some(at) => open.end + at,
            None => break,
        };
        let attributes = &body[open.end..header_end];
        let name = attribute(attributes, "name");
        let value_start = header_end + 1;
        // The invoke ends at its closing marker; a model that never closed it
        // ends it where the next invoke starts, or at the end of the block.
        let next_open = find_marker(body, value_start, Tag::Invoke, false);
        let close = find_marker(body, value_start, Tag::Invoke, true);
        let end = match (close, next_open) {
            (Some(close), Some(next)) => close.at.min(next.at),
            (Some(close), None) => close.at,
            (None, Some(next)) => next.at,
            (None, None) => body.len(),
        };
        from = match close {
            Some(close) if close.at <= end => close.end,
            _ => end,
        };
        if let Some(name) = name {
            invokes.push(Invoke {
                name,
                parameters: parse_parameters(&body[value_start..end.min(body.len())]),
            });
        }
    }
    invokes
}

fn parse_parameters(invoke_body: &str) -> Vec<(String, Option<String>, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(open) = find_marker(invoke_body, from, Tag::Parameter, false) {
        let Some(gt) = invoke_body[open.end..].find('>').map(|at| open.end + at) else {
            break;
        };
        let attributes = &invoke_body[open.end..gt];
        let Some(name) = attribute(attributes, "name") else {
            from = gt + 1;
            continue;
        };
        let string = attribute(attributes, "string");
        let value_start = gt + 1;
        let close = find_marker(invoke_body, value_start, Tag::Parameter, true);
        let next_open = find_marker(invoke_body, value_start, Tag::Parameter, false);
        let end = match (close, next_open) {
            (Some(close), Some(next)) => close.at.min(next.at),
            (Some(close), None) => close.at,
            (None, Some(next)) => next.at,
            (None, None) => invoke_body.len(),
        };
        from = match close {
            Some(close) if close.at <= end => close.end,
            _ => end,
        };
        // A duplicate parameter is a format error; keep the first, which is the
        // one the model committed to.
        if seen.insert(name.clone()) {
            out.push((name, string, invoke_body[value_start..end].to_string()));
        }
    }
    out
}

/// `name="value"` / `name='value'` / `name=value` inside a tag's attributes.
fn attribute(attributes: &str, key: &str) -> Option<String> {
    let lower = attributes.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(key) {
        let at = from + at;
        from = at + key.len();
        // Must be a whole attribute name, not the tail of another one.
        let before_ok = at == 0
            || attributes[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_whitespace() || c == '/');
        if !before_ok {
            continue;
        }
        let rest = attributes[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let quoted = &rest[quote.len_utf8()..];
            if let Some(end) = quoted.find(quote) {
                return Some(quoted[..end].to_string());
            }
            // Unterminated quote: take the rest.
            return Some(quoted.to_string());
        }
        let bare: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if bare.is_empty() {
            continue;
        }
        return Some(bare);
    }
    None
}

/// DSML separates literal strings from everything else so the model never has to
/// escape JSON inside a value: `string="true"` is the text itself, anything else
/// is JSON. A missing attribute, or JSON that does not parse, falls back to the
/// literal so a badly typed argument still reaches the tool.
fn arguments_json(parameters: &[(String, Option<String>, String)]) -> String {
    let mut map = serde_json::Map::new();
    for (name, string, value) in parameters {
        let decoded = match string.as_deref() {
            Some(flag) if flag.eq_ignore_ascii_case("true") => {
                serde_json::Value::String(value.clone())
            }
            _ => serde_json::from_str::<serde_json::Value>(value)
                .unwrap_or_else(|_| serde_json::Value::String(value.clone())),
        };
        map.insert(name.clone(), decoded);
    }
    serde_json::Value::Object(map).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools(names: &[&str]) -> DsmlTools {
        DsmlTools::new(names.iter().map(|name| name.to_string())).expect("a tool set")
    }

    fn scanner(names: &[&str]) -> DsmlScanner {
        DsmlScanner::new(tools(names))
    }

    /// Feed a whole message in one delta.
    fn run(names: &[&str], text: &str) -> (String, Vec<DsmlCall>) {
        let mut scanner = scanner(names);
        let mut step = scanner.push(text);
        let end = scanner.finish();
        step.extend(end);
        (step.text, step.calls)
    }

    /// The example from the upstream report, which is what karucode saw.
    const REPORTED: &str = "I will search several relevant directions first.\
        <||DSML||tool_calls><||DSML||invoke name=\"doc_knowledgebase\">\
        <||DSML||parameter name=\"query\" string=\"true\">bond detail fields display\
        </||DSML||parameter></||DSML||invoke></||DSML||tool_calls>";

    #[test]
    fn parses_the_block_from_the_upstream_report() {
        let (text, calls) = run(&["doc_knowledgebase"], REPORTED);
        assert_eq!(text, "I will search several relevant directions first.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "doc_knowledgebase");
        assert_eq!(
            calls[0].arguments,
            r#"{"query":"bond detail fields display"}"#
        );
    }

    #[test]
    fn parses_the_markup_karucode_actually_received() {
        // The screenshot in the bug report: full-width bars, newlines, and a
        // parameter value containing JSON with a quote in it.
        let body = "<\u{ff5c}DSML\u{ff5c}tool_calls>\n\
             <\u{ff5c}DSML\u{ff5c}invoke name=\"read_file\">\n\
             <\u{ff5c}DSML\u{ff5c}parameter name=\"path\" string=\"true\">crates/kc-server/src/routes/sessions.rs</\u{ff5c}DSML\u{ff5c}parameter>\n\
             </\u{ff5c}DSML\u{ff5c}invoke>\n\
             </\u{ff5c}DSML\u{ff5c}tool_calls>";
        let (text, calls) = run(&["read_file"], body);
        assert!(text.is_empty(), "no prose, got {text:?}");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(
            calls[0].arguments,
            r#"{"path":"crates/kc-server/src/routes/sessions.rs"}"#
        );
    }

    #[test]
    fn multiple_invokes_in_one_block_stay_separate_calls() {
        let body = "<||DSML||tool_calls>\
            <||DSML||invoke name=\"list_dir\"><||DSML||parameter name=\"path\" string=\"true\">.</||DSML||parameter></||DSML||invoke>\
            <||DSML||invoke name=\"git_status\"></||DSML||invoke>\
            </||DSML||tool_calls>";
        let (_, calls) = run(&["list_dir", "git_status"], body);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "list_dir");
        assert_eq!(calls[0].arguments, r#"{"path":"."}"#);
        assert_eq!(calls[1].name, "git_status");
        assert_eq!(calls[1].arguments, "{}");
    }

    #[test]
    fn string_false_parameters_are_json_and_string_true_are_literal() {
        let body = "<||DSML||tool_calls><||DSML||invoke name=\"read_file\">\
            <||DSML||parameter name=\"path\" string=\"true\">false</||DSML||parameter>\
            <||DSML||parameter name=\"limit\" string=\"false\">20</||DSML||parameter>\
            <||DSML||parameter name=\"flag\" string=\"false\">true</||DSML||parameter>\
            </||DSML||invoke></||DSML||tool_calls>";
        let (_, calls) = run(&["read_file"], body);
        let args: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["path"], serde_json::json!("false"));
        assert_eq!(args["limit"], serde_json::json!(20));
        assert_eq!(args["flag"], serde_json::json!(true));
    }

    #[test]
    fn a_missing_string_attribute_falls_back_to_json_then_to_text() {
        // vLLM records both shapes in production; dropping the parameter hands
        // the tool a call with no arguments at all.
        let body = "<||DSML||tool_calls><||DSML||invoke name=\"run_command\">\
            <||DSML||parameter name=\"command\">ls -la</||DSML||parameter>\
            <||DSML||parameter name=\"timeout_secs\">30</||DSML||parameter>\
            </||DSML||invoke></||DSML||tool_calls>";
        let (_, calls) = run(&["run_command"], body);
        let args: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["command"], serde_json::json!("ls -la"));
        assert_eq!(args["timeout_secs"], serde_json::json!(30));
    }

    #[test]
    fn a_block_naming_an_unknown_tool_stays_text() {
        // A model explaining the protocol must not fire a tool call, and an
        // invented tool must reach the operator rather than vanish.
        let (text, calls) = run(&["read_file"], REPORTED);
        assert!(calls.is_empty(), "must not run: {calls:?}");
        assert_eq!(text, REPORTED);
    }

    #[test]
    fn one_unknown_invoke_keeps_the_whole_block_as_text() {
        let body = "<||DSML||tool_calls>\
            <||DSML||invoke name=\"read_file\"><||DSML||parameter name=\"path\" string=\"true\">a</||DSML||parameter></||DSML||invoke>\
            <||DSML||invoke name=\"made_up\"></||DSML||invoke>\
            </||DSML||tool_calls>";
        let (text, calls) = run(&["read_file"], body);
        assert!(calls.is_empty(), "{calls:?}");
        assert_eq!(text, body);
    }

    #[test]
    fn no_tools_configured_means_no_recovery() {
        let mut scanner = DsmlScanner::new(DsmlTools::new(Vec::new()).unwrap_or_else(|| {
            // `new` returns None for an empty set; build one directly so this
            // test covers the "no tools" branch of the caller too.
            DsmlTools {
                names: HashSet::new(),
            }
        }));
        let step = scanner.push(REPORTED);
        assert!(step.calls.is_empty());
        assert_eq!(step.text, REPORTED);
    }

    #[test]
    fn markers_split_across_deltas_are_never_rendered_as_chat() {
        // Feed one character per delta: a naive implementation shows half a
        // marker and then has to retract it.
        let mut scanner = scanner(&["doc_knowledgebase"]);
        let mut text = String::new();
        let mut calls = Vec::new();
        for ch in REPORTED.chars() {
            let step = scanner.push(&ch.to_string());
            text.push_str(&step.text);
            calls.extend(step.calls);
        }
        let end = scanner.finish();
        text.push_str(&end.text);
        calls.extend(end.calls);

        assert_eq!(text, "I will search several relevant directions first.");
        assert_eq!(calls.len(), 1);
        assert!(!text.contains("DSML"), "{text}");
    }

    #[test]
    fn text_that_merely_starts_like_a_marker_is_released() {
        // `<` alone must not be held forever, and `<<<` is just text.
        let (text, calls) = run(&["read_file"], "use < and > for redirects, a < b");
        assert_eq!(text, "use < and > for redirects, a < b");
        assert!(calls.is_empty());

        let (text, _) = run(&["read_file"], "a <<not dsml>> b");
        assert_eq!(text, "a <<not dsml>> b");
    }

    #[test]
    fn a_partial_marker_at_the_end_of_a_message_is_flushed() {
        // The stream was cut mid-marker; the operator sees what arrived rather
        // than losing it.
        let mut scanner = scanner(&["read_file"]);
        let step = scanner.push("done.<|");
        assert_eq!(step.text, "done.");
        let end = scanner.finish();
        assert_eq!(end.text, "<|");
        assert!(end.calls.is_empty());
    }

    #[test]
    fn an_unterminated_block_is_returned_as_text() {
        let mut scanner = scanner(&["read_file"]);
        let partial = "<||DSML||tool_calls><||DSML||invoke name=\"read_file\">\
            <||DSML||parameter name=\"path\" string=\"true\">src/ma";
        let step = scanner.push(partial);
        assert!(step.text.is_empty(), "the block must be held, not shown");
        let end = scanner.finish();
        assert_eq!(end.text, partial);
        assert!(end.calls.is_empty());
    }

    #[test]
    fn text_after_a_block_is_kept_in_order() {
        let body = format!("before{REPORTED}after");
        let (text, calls) = run(&["doc_knowledgebase"], &body);
        assert_eq!(calls.len(), 1);
        assert!(text.starts_with("before"), "{text}");
        assert!(text.ends_with("after"), "{text}");
    }

    #[test]
    fn a_second_block_does_not_merge_into_the_first() {
        let one = "<||DSML||tool_calls><||DSML||invoke name=\"read_file\">\
            <||DSML||parameter name=\"path\" string=\"true\">a</||DSML||parameter>\
            </||DSML||invoke></||DSML||tool_calls>";
        let (_, calls) = run(&["read_file"], &format!("{one}{one}"));
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments, r#"{"path":"a"}"#);
        assert_eq!(calls[1].arguments, r#"{"path":"a"}"#);
    }

    #[test]
    fn lowercase_and_spaced_markers_are_accepted() {
        let body = "< || dsml || tool_calls >< || dsml || invoke name=\"read_file\">\
            < || dsml || parameter name=\"path\" string=\"true\">a</ || dsml || parameter>\
            </ || dsml || invoke></ || dsml || tool_calls>";
        let (_, calls) = run(&["read_file"], body);
        assert_eq!(calls.len(), 1, "a gateway may rewrite the marker");
        assert_eq!(calls[0].arguments, r#"{"path":"a"}"#);
    }

    #[test]
    fn a_parameter_containing_markup_like_text_keeps_its_value() {
        let body = "<||DSML||tool_calls><||DSML||invoke name=\"write_file\">\
            <||DSML||parameter name=\"content\" string=\"true\">a &lt; b > c</||DSML||parameter>\
            </||DSML||invoke></||DSML||tool_calls>";
        let (_, calls) = run(&["write_file"], body);
        let args: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["content"], serde_json::json!("a &lt; b > c"));
    }

    #[test]
    fn an_empty_block_is_not_a_call() {
        let (text, calls) = run(&["read_file"], "<||DSML||tool_calls></||DSML||tool_calls>");
        assert!(calls.is_empty());
        assert_eq!(text, "<||DSML||tool_calls></||DSML||tool_calls>");
    }

    #[test]
    fn attribute_parsing_requires_a_whole_key() {
        assert_eq!(attribute(" name=\"x\"", "name").as_deref(), Some("x"));
        assert_eq!(attribute(" fullname=\"x\"", "name"), None);
        assert_eq!(attribute(" name='x'", "name").as_deref(), Some("x"));
        assert_eq!(attribute(" name=x", "name").as_deref(), Some("x"));
        assert_eq!(attribute(" name=\"a b\"", "name").as_deref(), Some("a b"));
    }

    #[test]
    fn a_tool_prefixed_tag_name_is_not_tool_calls() {
        // `<|DSML|toolbox>` must not be read as `<|DSML|tool`.
        assert!(find_marker("<||DSML||toolbox>", 0, Tag::ToolCalls, false).is_none());
    }

    /// An unclosed block must not rescan the whole buffer on every delta.
    ///
    /// The shape that made this quadratic is an unclosed block full of `<` that
    /// never completes a marker: with a rescan from offset 0 on every delta, one
    /// megabyte of `< ` took 2.4 s in release (55 s in debug) and one megabyte
    /// of invoke markup 4.7 s (48 s). Both are driven here in small deltas,
    /// which is how a network stream arrives.
    #[test]
    fn an_unclosed_block_is_scanned_incrementally() {
        const TOTAL: usize = 512 * 1024;
        for (unit, chunk) in [
            ("< ", 256usize),
            ("<||DSML||parameter name=\"a\" string=\"true\">", 256),
        ] {
            let mut scanner = scanner(&["read_file"]);
            let _ = scanner.push("<||DSML||tool_calls><||DSML||invoke name=\"read_file\">");
            let filler: String = unit.repeat(chunk / unit.len() + 1);
            let filler = &filler[..chunk];

            let started = std::time::Instant::now();
            let mut fed = 0usize;
            while fed < TOTAL {
                let take = chunk.min(TOTAL - fed);
                let step = scanner.push(&filler[..take]);
                assert!(step.text.is_empty(), "an open block stays buffered");
                assert!(step.calls.is_empty());
                fed += take;
            }
            let elapsed = started.elapsed();

            // Still open at the end: an unterminated block is never a call.
            assert!(scanner.push("").calls.is_empty());
            // Two orders of magnitude above the fix (sub-millisecond per 512 KB
            // in release) and far below a rescan from offset 0 (seconds), so it
            // catches the quadratic shape without measuring the machine.
            assert!(
                elapsed < std::time::Duration::from_secs(2),
                "{TOTAL} bytes of unclosed block in {chunk}-byte deltas took {elapsed:?}; \
                 the open-block search must resume from where it stopped, not restart"
            );
        }
    }

    /// A closing marker split across delta boundaries must still be found when
    /// the search resumes from the end of the previous one.
    #[test]
    fn a_resumed_search_still_spots_a_marker_straddling_the_boundary() {
        let mut scanner = scanner(&["read_file"]);
        // Open the block, then feed only text, so the scanner's resume cursor
        // advances past everything it has seen.
        let head = "<||DSML||tool_calls><||DSML||invoke name=\"read_file\">\
                    <||DSML||parameter name=\"path\" string=\"true\">a.txt";
        assert!(scanner.push(head).text.is_empty());
        assert!(scanner.push(" and more prose").text.is_empty());

        // Now hand it a marker one character at a time, so the closing marker is
        // never whole inside a single delta.
        let tail = "</||DSML||parameter></||DSML||invoke></||DSML||tool_calls>";
        let mut calls = Vec::new();
        for ch in tail.chars() {
            let step = scanner.push(&ch.to_string());
            calls.extend(step.calls);
        }
        assert_eq!(
            calls.len(),
            1,
            "the split close must still be found: {calls:?}"
        );
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(
            calls[0].arguments, r#"{"path":"a.txt and more prose"}"#,
            "the value must keep everything between the tags"
        );
    }

    /// A `<` that is not a marker at all must not stop the scan from finding a
    /// real marker that follows it, even when both arrive in separate deltas.
    #[test]
    fn a_false_start_does_not_hide_a_later_closing_marker() {
        let mut scanner = scanner(&["read_file"]);
        let head = "<||DSML||tool_calls><||DSML||invoke name=\"read_file\">\
                    <||DSML||parameter name=\"path\" string=\"true\">a < b";
        assert!(scanner.push(head).text.is_empty());
        // A `<` that never becomes a tag, then the real close.
        assert!(scanner.push(" and <not a tag").text.is_empty());
        let step = scanner.push("></||DSML||parameter></||DSML||invoke></||DSML||tool_calls>");
        assert_eq!(step.calls.len(), 1, "{step:?}");
        assert_eq!(step.calls[0].name, "read_file");
        assert_eq!(
            step.calls[0].arguments,
            r#"{"path":"a < b and <not a tag>"}"#
        );
    }

    /// Feeding the same message in many small deltas must produce exactly what
    /// one big delta produces: the incremental search changes when the buffer is
    /// walked, never what is found.
    #[test]
    fn incremental_and_single_delta_scans_agree() {
        let messages = [
            REPORTED.to_string(),
            format!("before{REPORTED}after"),
            "<||DSML||tool_calls><||DSML||invoke name=\"read_file\">\
             <||DSML||parameter name=\"path\" string=\"true\">a.txt</||DSML||parameter>\
             </||DSML||invoke></||DSML||tool_calls>"
                .to_string(),
            "<||DSML||tool_calls><||DSML||invoke name=\"read_file\">\
             <||DSML||parameter name=\"path\" string=\"true\">unclosed"
                .to_string(),
        ];
        for message in messages {
            let (whole_text, whole_calls) = run(&["read_file", "doc_knowledgebase"], &message);
            let boundaries: Vec<usize> = message
                .char_indices()
                .map(|(at, _)| at)
                .chain(std::iter::once(message.len()))
                .collect();
            for step in [1usize, 2, 3, 7] {
                let mut scanner = scanner(&["read_file", "doc_knowledgebase"]);
                let (mut text, mut calls) = (String::new(), Vec::new());
                let mut at = 0usize;
                while boundaries[at] < message.len() {
                    let next = (at + step).min(boundaries.len() - 1);
                    let piece = &message[boundaries[at]..boundaries[next]];
                    at = next;
                    let pushed = scanner.push(piece);
                    text.push_str(&pushed.text);
                    calls.extend(pushed.calls);
                }
                let tail = scanner.finish();
                text.push_str(&tail.text);
                calls.extend(tail.calls);
                assert_eq!(text, whole_text, "step {step}: {message}");
                assert_eq!(calls, whole_calls, "step {step}: {message}");
            }
        }
    }
}

/// Text with any DSML tool-call markup removed.
///
/// [`DsmlScanner`] recovers calls while a message streams. This is the batched
/// equivalent for code that already holds a finished string and only wants the
/// prose - a title derived from a model's answer, for instance, where a block of
/// markup is not a title and the rail must not display one. The prose the model
/// wrote around the block is kept; nothing else is interpreted.
pub fn strip_tool_markup(text: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0;
    // Each iteration consumes one block, given that an opener exists at or after
    // the cursor. The text between the cursor and that opener is prose and stays.
    while let Some(open) = find_marker(text, cursor, Tag::ToolCalls, false) {
        out.push_str(&text[cursor..open.at]);
        match find_marker(text, open.end, Tag::ToolCalls, true) {
            Some(close) => cursor = close.end,
            None => {
                // Unterminated: everything from the opener on is markup.
                cursor = text.len();
                break;
            }
        }
    }
    out.push_str(&text[cursor..]);
    out
}

#[cfg(test)]
mod strip_tests {
    use super::*;

    const BLOCK: &str = "<||DSML||tool_calls><||DSML||invoke name=\"list_dir\">\
        <||DSML||parameter name=\"path\" string=\"true\">.</||DSML||parameter>\
        </||DSML||invoke></||DSML||tool_calls>";

    #[test]
    fn text_without_markup_is_returned_unchanged() {
        assert_eq!(strip_tool_markup("Fix the login bug"), "Fix the login bug");
        assert_eq!(strip_tool_markup(""), "");
        assert_eq!(strip_tool_markup("a < b and c > d"), "a < b and c > d");
    }

    #[test]
    fn prose_around_a_block_is_kept() {
        assert_eq!(
            strip_tool_markup(&format!("before{BLOCK}after")),
            "beforeafter"
        );
        assert_eq!(
            strip_tool_markup(&format!("Looking at it.{BLOCK}")),
            "Looking at it."
        );
        assert_eq!(strip_tool_markup(&format!("{BLOCK}after")), "after");
    }

    #[test]
    fn only_markup_leaves_nothing() {
        assert_eq!(strip_tool_markup(BLOCK), "");
    }

    #[test]
    fn every_block_is_removed() {
        let text = format!("a{BLOCK}b{BLOCK}c");
        assert_eq!(strip_tool_markup(&text), "abc");
    }

    #[test]
    fn an_unterminated_block_does_not_leak_its_markup() {
        // A model cut off mid-call: the opener and everything after it is markup.
        let text = "...<||DSML||tool_calls><||DSML||invoke name=\"read_file\">";
        assert_eq!(strip_tool_markup(text), "...");
    }

    #[test]
    fn the_full_width_spelling_is_stripped_too() {
        let text = "Working.<\u{ff5c}DSML\u{ff5c}tool_calls><\u{ff5c}DSML\u{ff5c}invoke name=\"read_file\">\
            </\u{ff5c}DSML\u{ff5c}invoke></\u{ff5c}DSML\u{ff5c}tool_calls>";
        assert_eq!(strip_tool_markup(text), "Working.");
    }
}
