// @kc-doc: tool-call-errors
// @kc-title: What a failed tool call tells the model
// @kc-summary: A call that fails because of how it was written comes back with a working version of itself, so the next attempt is correct instead of another guess.
// @kc-tags: tools, errors, agent, parsing, hints
// @kc-applies-to: tool:read_file, tool:run_command, tool:karucode_docs, route:POST /api/sessions/:id/messages
//
// Two very different things arrive at the model as a failed tool call, and it must
// not confuse them:
//
// * **a usage failure** - the call was written wrong: arguments that are not JSON,
//   a missing or mistyped argument, a name that is not a tool;
// * **an execution failure** - the call was right and did what it said: a test
//   suite exited non-zero, the command timed out, a person denied the approval.
//
// Only the first is about syntax the model can fix, so only the first is answered
// with the call it should have written. An execution failure never carries a "how
// to call this" hint, because the shape was never the problem.
//
// # A usage failure comes back with a working call
//
// The failed result names the problem and then shows the call that would parse:
//
// ```text
// invalid JSON arguments for tool `read_file`: ```json {"path": "a.txt",}```
// The arguments were not valid JSON. Re-issue the call as: {"name":"read_file","arguments":{"path":"a.txt"}}
// Required shape: {"name":"read_file","arguments":{"path":"<string>"}}
// ```
//
// The example is built from the tool's own JSON Schema, never invented: every
// `required` property is filled in, with the value the schema itself offers (an
// `enum` member or a `default`) and a type-correct placeholder otherwise. A tool
// that declares no properties gets no hint, because there is nothing to teach.
// The block is capped at `FAILURE_HINT_MAX_BYTES` (600) and the prose summary is
// dropped before the copyable JSON ever is.
//
// Mechanical JSON mistakes are repaired **for display only**: a code fence or a
// sentence around the object, trailing commas, single quotes, a double-encoded
// string. The repaired call is shown to the model, which re-issues it; the
// harness never executes a guess at what the model meant.
//
// | The model wrote | What it gets back |
// | --- | --- |
// | `{not json` | the parse error, the required shape |
// | ```` ```json {"path": "a.txt",}``` ```` | the repaired call, ready to copy |
// | `{"path": 7}` where `path` is a string | `` `path` must be string (JSON string), got integer `` |
// | `{}` with `path`/`content` required | `missing required argument `path``, `... `content`` |
// | `read_files` | `Did you mean `read_file`?` |
//
// # The schema is enforced, shallowly
//
// The arguments are checked against the schema the model was actually offered,
// before anything is approved or run: `required` presence, `enum` membership, a
// declared `type` (or a list, as in `["string","null"]`) and
// `additionalProperties: false`. `null` is not a universal bypass: it must be
// allowed by the declared type (for example `["string","null"]`) or by an enum
// containing `null`, and must still satisfy every other declared constraint.
// Richer keywords (`anyOf`, `$ref`, `format`,
// nested `required`) are left to the tool, because a wrong "invalid" verdict from
// the harness would block a call the tool would have accepted. A server that
// publishes a bare `{"type":"object"}` gets no second-guessing at all.
//
// # What a refusal from the tool itself still gets
//
// A tool that rejects the request after parsing it - `pattern may not be empty`,
// an unknown enum value the schema did not declare - reports it as a bad request,
// and that result *also* carries the working call. That covers the built-in tools
// and any plugin or MCP server that reports an invalid-parameter error.
//
// An approval denial or a policy refusal is an execution outcome, not a parse
// failure: it says who refused and what to change in the policy, and shows no
// argument example.

use crate::approval::{ApprovalDecision, ApprovalKind, ApprovalRequest};
use crate::config::Limits;
use crate::error::Result;
use crate::events::{Event, EventSink};
use crate::ids::{AgentId, RunId, SessionId};
use crate::paths::Workspace;
use crate::settings::Settings;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    /// Inspects state and never changes the workspace.
    ReadOnly,
    /// Creates or modifies files inside the workspace.
    Mutating,
    /// Runs arbitrary commands as the server user. Not sandboxed.
    Shell,
    /// Spawns or controls subagents.
    Delegate,
    /// Calls out to a plugin or MCP server.
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolOrigin {
    #[default]
    Builtin,
    Plugin {
        name: String,
    },
    Mcp {
        server: String,
    },
}

impl ToolOrigin {
    pub fn label(&self) -> String {
        match self {
            ToolOrigin::Builtin => "builtin".into(),
            ToolOrigin::Plugin { name } => format!("plugin:{name}"),
            ToolOrigin::Mcp { server } => format!("mcp:{server}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub origin: ToolOrigin,
}

/// Longest a [`ToolSpec::failure_hint`] may be.
///
/// The hint is appended to a failed tool result, which is itself bounded by
/// `limits.max_tool_output_bytes` (and by the turn's remaining budget, whose
/// floor is a couple of kilobytes). A schema the size of `spawn_agents`' must
/// not push the real error out of a tight budget, so the hint is capped here and
/// the *summary* is what gets dropped first - never the JSON example.
pub const FAILURE_HINT_MAX_BYTES: usize = 600;

/// Build the smallest argument object a JSON Schema says the tool accepts.
///
/// Only required properties are filled in, in the order the schema declares
/// them; a tool with no required property gets its first declared one, so the
/// example is a concrete call rather than `{}`. Values come from the schema
/// itself (`enum`, `default`, `examples`, `minimum`) and are otherwise a
/// type-correct placeholder (`"<string>"`, `1`, `true`). The result is a shape
/// the model can copy, not a guess at the user's task.
fn example_object(schema: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return serde_json::Value::Object(out);
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    for key in props.keys() {
        if required.contains(&key.as_str()) {
            out.insert(key.clone(), example_value(&props[key]));
        }
    }
    if out.is_empty() {
        if let Some((key, value)) = props.iter().next() {
            out.insert(key.clone(), example_value(value));
        }
    }
    serde_json::Value::Object(out)
}

fn example_value(schema: &serde_json::Value) -> serde_json::Value {
    // An explicit value the schema offers always beats a placeholder: a model
    // copying `{"action":"list"}` is right, while copying `"<string>"` is not.
    for key in ["enum", "default", "examples"] {
        let value = schema.get(key);
        if key == "enum" || key == "examples" {
            if let Some(first) = value.and_then(|v| v.as_array()).and_then(|a| a.first()) {
                return first.clone();
            }
        } else if let Some(default) = value {
            return default.clone();
        }
    }
    match schema.get("type").and_then(|t| t.as_str()) {
        Some("integer") | Some("number") => schema
            .get("minimum")
            .cloned()
            .unwrap_or_else(|| serde_json::json!(1)),
        Some("boolean") => serde_json::json!(true),
        Some("array") => {
            let item = schema
                .get("items")
                .map(example_value)
                .unwrap_or_else(|| serde_json::json!("<string>"));
            serde_json::json!([item])
        }
        Some("object") => example_object(schema),
        Some("null") => serde_json::Value::Null,
        _ => serde_json::json!("<string>"),
    }
}

/// How many properties the one-line argument summary may name.
const HINT_PROPERTY_LIMIT: usize = 6;

/// One tool call rendered the way a model should write it: `name` first, no
/// map-order surprises.
///
/// `serde_json::json!` on a `Map` sorts keys, which turns the example into
/// `{"arguments":…,"name":…}` - still valid, but not the shape the wire format
/// uses, and a model copying it is copying a coincidence.
pub fn call_snippet(name: &str, arguments: &serde_json::Value) -> String {
    format!(
        "{{\"name\":{},\"arguments\":{}}}",
        serde_json::Value::String(name.to_string()),
        arguments
    )
}

impl ToolSpec {
    /// A complete argument object the schema accepts, with every required key.
    pub fn example_arguments(&self) -> serde_json::Value {
        example_object(&self.parameters)
    }

    /// One JSON call an agent can copy verbatim.
    pub fn example_call(&self) -> String {
        call_snippet(&self.name, &self.example_arguments())
    }

    /// `required: path (string); optional: offset (integer), limit (integer)`.
    ///
    /// Property *names and JSON types* are what a model gets wrong, so those come
    /// first; the description is not repeated here (it is already in the tool
    /// schema the provider sent, and would triple the size of this line).
    fn arguments_summary(&self) -> Option<String> {
        let props = self.parameters.get("properties")?.as_object()?;
        if props.is_empty() {
            return None;
        }
        let required: Vec<&str> = self
            .parameters
            .get("required")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let ty = |key: &str| {
            props
                .get(key)
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("any")
                .to_string()
        };
        let mut parts = Vec::new();
        if !required.is_empty() {
            parts.push(format!(
                "required: {}",
                required
                    .iter()
                    .map(|key| format!("{key} ({})", ty(key)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let optional: Vec<String> = props
            .keys()
            .filter(|key| !required.contains(&key.as_str()))
            .take(HINT_PROPERTY_LIMIT)
            .map(|key| format!("{key} ({})", ty(key)))
            .collect();
        if !optional.is_empty() {
            parts.push(format!(
                "optional: {}{}",
                optional.join(", "),
                if props.len() > required.len() + optional.len() {
                    ", ..."
                } else {
                    ""
                }
            ));
        }
        Some(parts.join("; "))
    }

    /// The block appended to a failed call so the next attempt has the right
    /// shape: what the arguments are and a copyable call that satisfies them.
    ///
    /// Empty when the schema carries no properties (a tool that takes nothing
    /// has nothing to teach, and the failure is not a parsing failure).
    pub fn failure_hint(&self) -> String {
        let summary = self.arguments_summary();
        let example = self.example_call();
        if summary.is_none()
            && self
                .example_arguments()
                .as_object()
                .is_some_and(|o| o.is_empty())
        {
            return String::new();
        }
        let mut hint = format!("[how to call `{}`: {example}", self.name);
        if let Some(summary) = summary {
            // The example is the copyable part; drop the summary before ever
            // cutting the JSON, and only cut the JSON as a last resort.
            if hint.len() + summary.len() + 2 <= FAILURE_HINT_MAX_BYTES {
                hint.push_str(" — ");
                hint.push_str(&summary);
            }
        }
        hint.push(']');
        if hint.len() > FAILURE_HINT_MAX_BYTES {
            let mut end = FAILURE_HINT_MAX_BYTES.saturating_sub(4);
            while end > 0 && !hint.is_char_boundary(end) {
                end -= 1;
            }
            hint.truncate(end);
            hint.push_str("...]");
        }
        hint
    }

    /// Check an already-parsed argument object against the schema the model was
    /// given, returning every problem it can prove.
    ///
    /// Deliberately shallow: `required` presence, `enum` membership, a declared
    /// `type` (or a list of them, as some schemas write `["string","null"]`) and
    /// `additionalProperties: false`. Anything richer (`anyOf`, `$ref`, `format`,
    /// nested `required`) is left to the tool, because a wrong "invalid" answer
    /// from the harness would block a call the tool would have accepted.
    pub fn validate_arguments(
        &self,
        args: &serde_json::Value,
    ) -> std::result::Result<(), Vec<String>> {
        let mut problems = Vec::new();
        if self.parameters.get("type").and_then(|t| t.as_str()) != Some("object")
            && self.parameters.get("properties").is_none()
        {
            return Ok(());
        }
        let Some(object) = args.as_object() else {
            return Err(vec![format!(
                "`{}` takes a JSON object, got {}",
                self.name,
                json_type(args)
            )]);
        };
        let props = self
            .parameters
            .get("properties")
            .and_then(|p| p.as_object());
        for key in self
            .parameters
            .get("required")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default()
        {
            if !object.contains_key(key) {
                problems.push(format!("missing required argument `{key}`"));
            }
        }
        if let Some(props) = props {
            for (key, value) in object {
                let Some(schema) = props.get(key) else {
                    if self.parameters.get("additionalProperties")
                        == Some(&serde_json::json!(false))
                    {
                        problems.push(format!("unknown argument `{key}`"));
                    }
                    continue;
                };
                if let Some(allowed) = schema.get("enum").and_then(|e| e.as_array()) {
                    if !allowed.contains(value) {
                        problems.push(format!(
                            "`{key}` must be one of {}, got {}",
                            allowed
                                .iter()
                                .map(|v| v.to_string())
                                .collect::<Vec<_>>()
                                .join(", "),
                            value
                        ));
                        continue;
                    }
                }
                if let Some(declared) = schema.get("type") {
                    let types: Vec<&str> = match declared {
                        serde_json::Value::String(s) => vec![s.as_str()],
                        serde_json::Value::Array(a) => {
                            a.iter().filter_map(|t| t.as_str()).collect()
                        }
                        _ => Vec::new(),
                    };
                    if !types.is_empty() && !types.iter().any(|t| matches_json_type(value, t)) {
                        problems.push(format!(
                            "`{key}` must be {} (JSON {}), got {}",
                            types.join(" or "),
                            types.join("|"),
                            json_type(value)
                        ));
                    }
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

fn json_type(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(n) => {
            if n.is_f64() && n.as_i64().is_none() {
                "number"
            } else {
                "integer"
            }
        }
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn matches_json_type(value: &serde_json::Value, declared: &str) -> bool {
    let actual = json_type(value);
    match declared {
        // Every JSON integer is a valid number; the reverse is not true.
        "number" => actual == "number" || actual == "integer",
        other => actual == other,
    }
}

/// Try to turn a malformed argument string into the call the model meant.
///
/// A model writes tool arguments as text long before a parser sees them, so the
/// failures are almost always mechanical: a code fence or a sentence around the
/// object, a trailing comma, single quotes, an unquoted key. Repairing them here
/// means the failure output can show *the same call with the syntax fixed*
/// instead of an abstract schema, and a repair is only reported when it parses.
///
/// `None` when nothing parses. The result is never executed: it is shown to the
/// model, which re-issues the call itself.
pub fn repair_arguments(raw: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        // One level of unwrapping covers the model that JSON-encoded its
        // argument object a second time (`"{\"path\":\"a.txt\"}"`): the string is
        // valid JSON, so the plain parse would hand back a string and the call
        // would be rejected further down for the wrong reason.
        if let serde_json::Value::String(inner) = &value {
            if let Ok(serde_json::Value::Object(obj)) =
                serde_json::from_str::<serde_json::Value>(inner)
            {
                return Some(serde_json::Value::Object(obj));
            }
        }
        return Some(value);
    }
    let candidate = extract_json_span(raw);
    // Apply the mechanical fixes cumulatively, then try each stage: a model that
    // wrote valid JSON inside a fence needs only the first, one that also used
    // single quotes needs the second.
    let mut stages = vec![candidate.clone()];
    let stripped = strip_trailing_commas(&candidate);
    if stripped != candidate {
        stages.push(stripped);
    }
    for stage in stages.clone() {
        let quoted = requote_single_strings(&stage);
        if quoted != stage {
            stages.push(requote_single_strings(&strip_trailing_commas(&quoted)));
        }
    }
    for stage in stages {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&stage) {
            return Some(value);
        }
        // `{"arguments": "{\"path\":\"a\"}"}`: a double-encoded object.
        if let Ok(serde_json::Value::String(inner)) =
            serde_json::from_str::<serde_json::Value>(&stage)
        {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&inner) {
                return Some(value);
            }
        }
    }
    None
}

/// The first balanced `{...}` or `[...]` in `raw`, quotes respected.
fn extract_json_span(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some(start) = trimmed.find(['{', '[']) else {
        return trimmed.to_string();
    };
    let open = trimmed.as_bytes()[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let bytes = trimmed.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b if b == open => depth += 1,
            b if b == close => {
                depth -= 1;
                if depth == 0 {
                    return trimmed[start..=i].to_string();
                }
            }
            _ => {}
        }
    }
    trimmed[start..].to_string()
}

/// Drop `,]` and `,}` (and their whitespace-separated forms), which are the
/// single most common hand-written JSON mistake.
fn strip_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            out.push(byte as char);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                out.push('"');
            }
            b',' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
                    // Skip the comma and the whitespace it carried.
                    i = j;
                    continue;
                }
                out.push(',');
            }
            other => out.push(other as char),
        }
        i += 1;
    }
    out
}

/// Rewrite `'value'` / `'key':` as JSON double-quoted strings.
///
/// Only for text that is not already valid JSON, so a legitimate apostrophe
/// inside a double-quoted string is never touched: strings that start with `"`
/// are copied verbatim.
fn requote_single_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            out.push(c);
            let mut escaped = false;
            for c in chars.by_ref() {
                out.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    break;
                }
            }
            continue;
        }
        if c == '\'' {
            out.push('"');
            let mut escaped = false;
            for c in chars.by_ref() {
                if escaped {
                    out.push('\\');
                    out.push(c);
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '\'' {
                    out.push('"');
                    break;
                } else if c == '"' {
                    out.push_str("\\\"");
                } else {
                    out.push(c);
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The tool name in `names` closest to `wanted`, when it is close enough to be
/// the name the model meant rather than a different tool entirely.
///
/// Plain Levenshtein distance, bounded to a small budget: the point is to catch
/// `read_files` / `Read_File` / `searchfiles`, not to fuzzy-match unrelated
/// tools. `None` when nothing is within a third of the wanted name's length.
pub fn closest_tool_name<'a, I>(wanted: &str, names: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let lower = wanted.to_ascii_lowercase();
    let budget = (lower.chars().count() / 3).max(1);
    names
        .into_iter()
        .map(|name| {
            let distance = levenshtein(&lower, &name.to_ascii_lowercase());
            (distance, name)
        })
        .filter(|(distance, _)| *distance <= budget)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, name)| name.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ac) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            let cost = usize::from(ac != bc);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    pub call_id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

impl ToolCallRequest {
    pub fn arg_str(&self, key: &str) -> Result<&str> {
        self.arguments
            .get(key)
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::Error::BadRequest(format!("missing string argument `{key}`")))
    }

    pub fn opt_str(&self, key: &str) -> Option<&str> {
        self.arguments.get(key).and_then(|v| v.as_str())
    }

    pub fn opt_u64(&self, key: &str) -> Option<u64> {
        self.arguments.get(key).and_then(|v| v.as_u64())
    }

    pub fn opt_bool(&self, key: &str) -> Option<bool> {
        self.arguments.get(key).and_then(|v| v.as_bool())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub ok: bool,
    pub content: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
    /// Images this result wants the model to look at. Only the built-in tools
    /// produce them (plugin/MCP results decode with an empty list), and only when
    /// [`ToolCtx::supports_images`] says the active model accepts image input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<crate::message::ImageAttachment>,
}

impl ToolOutcome {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            ok: true,
            content: content.into(),
            truncated: false,
            meta: None,
            images: Vec::new(),
        }
    }

    pub fn err(content: impl Into<String>) -> Self {
        Self {
            ok: false,
            content: content.into(),
            truncated: false,
            meta: None,
            images: Vec::new(),
        }
    }

    pub fn with_meta(mut self, meta: serde_json::Value) -> Self {
        self.meta = Some(meta);
        self
    }

    pub fn with_images(mut self, images: Vec<crate::message::ImageAttachment>) -> Self {
        self.images = images;
        self
    }

    pub fn truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }

    pub fn from_result(r: Result<String>) -> Self {
        match r {
            Ok(s) => ToolOutcome::ok(s),
            Err(e) => ToolOutcome::err(e.to_string()),
        }
    }
}

#[async_trait]
pub trait ApprovalGate: Send + Sync {
    /// Block until the user answers (or the policy decides automatically).
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision;
}

/// Thin link between the caller and the approval gate, used by tools so they can
/// ask without knowing about the store or the UI.
#[derive(Clone)]
pub struct ApprovalHandle {
    gate: Arc<dyn ApprovalGate>,
    pub session_id: SessionId,
    pub run_id: RunId,
    pub agent_id: Option<AgentId>,
}

impl ApprovalHandle {
    pub fn new(
        gate: Arc<dyn ApprovalGate>,
        session_id: SessionId,
        run_id: RunId,
        agent_id: Option<AgentId>,
    ) -> Self {
        Self {
            gate,
            session_id,
            run_id,
            agent_id,
        }
    }

    pub async fn ask(
        &self,
        kind: ApprovalKind,
        title: impl Into<String>,
        detail: impl Into<String>,
        payload: serde_json::Value,
    ) -> ApprovalDecision {
        let request = ApprovalRequest {
            id: crate::ids::ApprovalId::new().0,
            kind,
            title: title.into(),
            detail: detail.into(),
            payload,
            session_id: self.session_id.0.clone(),
            run_id: self.run_id.0.clone(),
            agent_id: self.agent_id.as_ref().map(|a| a.0.clone()),
        };
        self.gate.request(request).await
    }
}

/// A file's state immediately before a mutating tool changed it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CapturedFile {
    /// False when the file did not exist, so undoing the change removes it.
    pub existed: bool,
    /// The bytes it had. Empty when `existed` is false or when the capture was
    /// refused (binary content, or larger than the cap).
    pub content: String,
    /// True when the file existed but its bytes could not be kept.
    pub failed: bool,
}

impl CapturedFile {
    /// The normal case: the file did not exist.
    pub fn absent() -> Self {
        Self::default()
    }

    pub fn text(content: impl Into<String>) -> Self {
        Self {
            existed: true,
            content: content.into(),
            failed: false,
        }
    }

    /// The file exists but is too large or not text; a rewind cannot restore it.
    pub fn uncapturable() -> Self {
        Self {
            existed: true,
            content: String::new(),
            failed: true,
        }
    }
}

/// Capture `abs` for the rewind journal.
///
/// A file that does not exist is [`CapturedFile::absent`]; a directory, a binary
/// file or one over [`crate::checkpoint::MAX_SNAPSHOT_BYTES`] is
/// [`CapturedFile::uncapturable`], because restoring half a file is worse than
/// admitting the rewind cannot restore it.
pub fn capture_file(abs: &std::path::Path) -> CapturedFile {
    let meta = match std::fs::symlink_metadata(abs) {
        Ok(meta) => meta,
        Err(_) => return CapturedFile::absent(),
    };
    if !meta.is_file() || meta.len() > crate::checkpoint::MAX_SNAPSHOT_BYTES as u64 {
        return CapturedFile::uncapturable();
    }
    match std::fs::read(abs) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => CapturedFile::text(text),
            Err(_) => CapturedFile::uncapturable(),
        },
        Err(_) => CapturedFile::uncapturable(),
    }
}

/// Records the before-image of every file a mutating tool changes.
///
/// The runtime builds one per run and hands it to each [`ToolCtx`]. Built-in file
/// mutations call [`ToolCtx::capture_checkpoint`] immediately before writing and
/// refuse when that managed capture fails. The lower-level best-effort
/// [`JournalHandle::record`] remains available to callers that explicitly permit
/// an unrestorable change.
#[async_trait]
pub trait JournalHandle: Send + Sync {
    /// Capture `rel` (workspace-relative) because `tool` is about to change it.
    ///
    /// Best-effort: a journal that cannot be written leaves the change
    /// unrestorable, which the rewind reports, rather than failing the write.
    async fn record(&self, rel: &str, tool: &str);

    /// Capture `rel` and report whether the capture landed, for a caller that
    /// needs to refuse the write when it did not.
    async fn capture_path(&self, rel: &str, tool: &str) -> bool;
}

/// Everything a tool is allowed to know about its execution context.
#[derive(Clone)]
pub struct ToolCtx {
    pub workspace: Arc<Workspace>,
    pub cancel: CancellationToken,
    pub approvals: ApprovalHandle,
    pub events: EventSink,
    pub settings: Arc<dyn Settings>,
    /// Subagent nesting depth; 0 for the main agent.
    pub depth: u8,
    /// Whether the model running this turn accepts image input.
    ///
    /// The runtime resolves it from the session's effective model
    /// (`model.supports_images`), so a session pinned to a vision model gets
    /// images even when the install-wide default is a text-only one. A tool that
    /// would otherwise return pixels must check this and explain the limitation
    /// instead of sending an image the endpoint will reject.
    pub supports_images: bool,
    /// Where a mutating tool records the bytes it is about to replace, so a
    /// rewind can put the file back. `None` outside a run (a test harness, a
    /// tool invoked directly), in which case the change simply cannot be undone.
    pub checkpoints: Option<Arc<dyn JournalHandle>>,
}

impl ToolCtx {
    pub fn emit(&self, event: Event) {
        (self.events)(event);
    }

    pub async fn limits(&self) -> Limits {
        self.settings.limits().await
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Record the state of `rel` before a mutating tool changes it.
    ///
    /// Best-effort by design: a journal that cannot be written must not fail the
    /// write the user asked for. The rewind then reports that path as
    /// unrestorable rather than silently restoring the wrong bytes - which is
    /// why this returns `()` and not `Result`. A tool that *must* know can call
    /// [`JournalHandle::capture_path`] itself.
    pub async fn record_checkpoint(&self, rel: &str, tool: &str) {
        if let Some(journal) = &self.checkpoints {
            journal.record(rel, tool).await;
        }
    }

    /// Capture the before-image and say whether it is safe for a managed write to
    /// proceed. Direct tool use without a journal deliberately keeps its existing
    /// non-journaled behaviour; an attached journal must confirm the capture.
    pub async fn capture_checkpoint(&self, rel: &str, tool: &str) -> bool {
        match &self.checkpoints {
            Some(journal) => journal.capture_path(rel, tool).await,
            None => true,
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }

    /// Whether the runtime must collect an approval before invoking this tool.
    ///
    /// Built-in mutating/shell tools gate *themselves* (they can show the exact
    /// command or file diff), so the runtime only consults this for tools from
    /// plugins and MCP servers. Returning `true` means the runtime asks the
    /// approval policy first; `false` means the tool is exempt (for example an
    /// MCP server configured with `require_approval = false`).
    fn requires_approval(&self) -> bool {
        !matches!(self.class(), ToolClass::ReadOnly)
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome;
}

impl std::fmt::Debug for dyn Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tool({})", self.spec().name)
    }
}

/// The set of tools exposed to the model. Ordered by name for stable prompts.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.spec().name;
        self.tools.insert(name, tool);
    }

    pub fn register_all(&mut self, tools: Vec<Arc<dyn Tool>>) {
        for t in tools {
            self.register(t);
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn remove(&mut self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.remove(name)
    }

    pub fn remove_prefix(&mut self, prefix: &str) {
        self.tools.retain(|k, _| !k.starts_with(prefix));
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// The configuration key whose bound cuts a tool result that has no narrower
/// request of its own.
///
/// Every truncation marker names it: "raise the limit" without a key name is a
/// dead end for a model that cannot edit the operator's config, while the name
/// lets it tell the user exactly what to change.
pub const TOOL_OUTPUT_LIMIT_KEY: &str = "limits.max_tool_output_bytes";

/// What a generic cut tells the model to do next: this is the part that turns
/// "your result is incomplete" into a next call. A tool with a better answer
/// passes its own sentence to [`truncate_output_with_advice`].
pub const TOOL_OUTPUT_LIMIT_ADVICE: &str = "re-issue the call with a narrower request \
     (offset=/limit= where the tool takes them, a smaller max_results, a narrower path, glob or \
     pattern) or raise limits.max_tool_output_bytes";

/// How far a cut may move to land on a line boundary before the payload counts as
/// long-line data. A 12 KiB half-window that has no newline within 2 KiB of its
/// middle is minified output or a single record; there the honest report is bytes.
const MAX_LINE_SEAM_SHIFT: usize = 2 * 1024;

/// Geometry of one truncation, in the units a reader can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cut {
    /// Bytes of the complete result.
    pub total_bytes: usize,
    /// Lines of the complete result.
    pub total_lines: usize,
    /// Bytes kept from the head.
    pub head_bytes: usize,
    /// Bytes kept from the tail.
    pub tail_bytes: usize,
    /// Bytes dropped from the middle.
    pub dropped_bytes: usize,
    /// Whole lines dropped from the middle; `0` when the seam is not line-aligned.
    pub dropped_lines: usize,
    /// 1-based number of the first dropped line, when the cut is line-aligned.
    pub first_dropped_line: Option<usize>,
    /// Both seams sit on line boundaries, so no line is half-delivered.
    pub line_aligned: bool,
}

/// Truncate a tool result to a byte budget without splitting a UTF-8 character.
///
/// The complete return value - marker included - fits the budget. Keeping a tail
/// matters for shell output: a final runtime-budget cut must not remove the error
/// that a command printed to stderr after a noisy stdout stream.
///
/// The marker is not just a flag. It reports the size of both what was kept and
/// what was dropped, in bytes *and* lines, and says what to do next - because a
/// cut the model cannot resume from costs a whole re-read (or a wrong answer).
/// Line-oriented payloads are cut on line boundaries, so a numbered region is
/// never spliced mid-line; the reported line numbers are then exact. Very small
/// budgets get the most compact marker that still fits, and a budget too small
/// for any marker keeps a valid prefix (`cut` still reports it).
pub fn truncate_output(text: &str, max_bytes: usize) -> (String, bool) {
    truncate_output_with_advice(text, max_bytes, TOOL_OUTPUT_LIMIT_ADVICE)
}

/// [`truncate_output`] with the caller's own "what to do next" sentence.
///
/// `advice` must name a real next call or the key that governs the bound; it is
/// reserved out of the byte budget like the rest of the marker. A budget too
/// small for the whole sentence does not drop it: the compact markers keep the
/// advice's first clause, or the key name it ends with, whichever fits - and a
/// smaller marker is preferred to a richer one that would lose those words,
/// because "raise the limit" is a next step where an operator's sentence is the
/// only one the model can pass on for *this* tool.
pub fn truncate_output_with_advice(text: &str, max_bytes: usize, advice: &str) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    // The marker explains a cut; it must not become most of what is left. A tiny
    // budget gets the compact marker so the content - and the tail, where a
    // command's error usually is - keeps the room.
    let marker_budget = (max_bytes / 2).max(80);
    // Each attempt renders its own marker from the geometry that attempt actually
    // produces, so the byte and line numbers in the text always describe the
    // splice the reader is looking at. Attempts are compared rather than taken
    // first-fit: keeping the caller's advice is worth a shorter geometry, and a
    // later attempt that keeps more of it wins.
    let mut best: Option<((Advice, u8), String)> = None;
    for detail in [Detail::Full, Detail::Short, Detail::Minimal] {
        let Some(candidate) = assemble_cut(text, max_bytes, advice, detail, marker_budget) else {
            continue;
        };
        let rank = (candidate.advice, detail.richness());
        if best.as_ref().is_none_or(|(best_rank, _)| rank > *best_rank) {
            best = Some((rank, candidate.text));
        }
    }
    if let Some((_, out)) = best {
        return (out, true);
    }
    // No marker fits: a valid prefix is all a budget this small can carry, and
    // the caller still reports `cut`. This is the only remaining silent case and
    // it needs a budget below one short sentence.
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// How much the marker says, in decreasing order of usefulness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    Full,
    Short,
    Minimal,
}

impl Detail {
    /// How much geometry this detail reports, compared when two attempts both
    /// keep the same amount of the caller's advice.
    fn richness(self) -> u8 {
        match self {
            Detail::Full => 2,
            Detail::Short => 1,
            Detail::Minimal => 0,
        }
    }
}

/// How much of the caller's own sentence a marker carried.
///
/// Ordered by what a reader can do with it: the whole sentence names the call
/// that continues *this* result, a shortened form still says what the tool needs
/// and how to shorten it, and the key name alone only tells an operator what to
/// change. A marker that kept less advice is only preferred when nothing else
/// fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Advice {
    None,
    Key,
    Short,
    Whole,
}

/// One candidate result and what of the advice it kept.
struct Candidate {
    text: String,
    advice: Advice,
}

/// One candidate result: pick a seam that leaves room for a marker of `detail`,
/// render that marker, and accept it when the whole thing fits the budget.
///
/// The marker's own length depends on the numbers it reports, which depend on the
/// room it leaves - so the length is estimated, then recomputed from what was
/// actually rendered until it settles. Every step is a pure function of
/// `(text, max_bytes, advice)`, which keeps a cut result byte-identical between
/// runs: provider prefix caching depends on that.
fn assemble_cut(
    text: &str,
    max_bytes: usize,
    advice: &str,
    detail: Detail,
    marker_budget: usize,
) -> Option<Candidate> {
    let mut reserve = match detail {
        Detail::Full => 480,
        Detail::Short => 160,
        Detail::Minimal => 64,
    };
    for _ in 0..4 {
        let (head_end, tail_start, cut) = cut_at(text, max_bytes.saturating_sub(reserve));
        let marker = render_marker(&cut, advice, detail, marker_budget);
        if marker.text.len() > marker_budget {
            return None;
        }
        let total = head_end + marker.text.len() + (text.len() - tail_start);
        if total <= max_bytes {
            return Some(Candidate {
                text: format!(
                    "{}{}{}",
                    &text[..head_end],
                    marker.text,
                    &text[tail_start..]
                ),
                advice: marker.advice,
            });
        }
        if marker.text.len() <= reserve {
            // Shrinking the estimate cannot help any more; this detail is too
            // verbose for the budget.
            return None;
        }
        reserve = marker.text.len();
    }
    None
}

/// Where a cut with `payload` bytes for content lands, and what it drops.
fn cut_at(text: &str, payload: usize) -> (usize, usize, Cut) {
    let payload = payload.min(text.len());
    let head_target = payload / 2;
    let tail_bytes = payload - head_target;
    let tail_target = text.len().saturating_sub(tail_bytes);
    let mut tail_start =
        line_start_near(text, tail_target).unwrap_or_else(|| floor_char(text, tail_target));
    let mut head_end = line_end_near(text, head_target.min(tail_start))
        .unwrap_or_else(|| floor_char(text, head_target));
    head_end = head_end.min(tail_start);

    // A body that was already cut carries a marker of its own. Keeping it would
    // stack two markers around one seam - the older numbers describe a splice the
    // reader can no longer see - so the seam moves past it. The kept bytes are
    // still a contiguous head and tail; only the superseded marker is dropped.
    if let Some((start, end)) = marker_overlapping(text, head_end, tail_start) {
        if start < head_end {
            // The old marker sits inside the kept head. Never spend the whole head
            // on removing it: what the reader can still use beats a tidier count.
            if start > 0 {
                head_end = start;
            }
        } else {
            // It starts inside the kept tail: resume after it, at the next line.
            tail_start = end;
            while text.as_bytes().get(tail_start) == Some(&b'\n') {
                tail_start += 1;
            }
        }
    }

    let tail_line_start = tail_start == 0
        || tail_start == text.len()
        || text.as_bytes().get(tail_start - 1) == Some(&b'\n');
    let head_line_end = head_end == 0
        || text.as_bytes().get(head_end.wrapping_sub(1)) == Some(&b'\n')
        || head_end == text.len();
    let line_aligned = head_line_end && tail_line_start;

    let dropped = text.get(head_end..tail_start).unwrap_or("");
    let mut dropped_lines = if line_aligned {
        dropped.matches('\n').count()
    } else {
        0
    };
    // A final partial line (no trailing newline) is still a whole line to a
    // reader, and `str::lines` counts it as one.
    if line_aligned && tail_start == text.len() && !dropped.is_empty() && !dropped.ends_with('\n') {
        dropped_lines += 1;
    }
    let cut = Cut {
        total_bytes: text.len(),
        total_lines: text.lines().count(),
        head_bytes: head_end,
        tail_bytes: text.len() - tail_start,
        dropped_bytes: dropped.len(),
        dropped_lines,
        first_dropped_line: line_aligned.then(|| 1 + text[..head_end].matches('\n').count()),
        line_aligned: line_aligned && head_end < tail_start,
    };
    (head_end, tail_start, cut)
}

/// The `(start, end)` of a truncation marker this module wrote that the kept head
/// or tail of a re-cut would otherwise include.
///
/// Only the exact shape [`render_marker`] emits is recognised - a blank line, the
/// `[truncated:` opener and the closing bracket - so ordinary text that merely
/// mentions the word is left alone.
fn marker_overlapping(text: &str, head_end: usize, tail_start: usize) -> Option<(usize, usize)> {
    const OPENER: &str = "\n\n[truncated:";
    let mut from = 0usize;
    while let Some(pos) = text[from..].find(OPENER) {
        let start = from + pos;
        let body = start + OPENER.len();
        let close = text[body..].find(']')?;
        let end = body + close + 1;
        if start < head_end || end > tail_start {
            return Some((start, end));
        }
        from = end;
    }
    None
}

/// The marker text for one cut at one level of detail, and how much of the
/// caller's advice it kept.
///
/// The `Full` arms are long enough to carry the whole sentence. The compact arms
/// have to earn their room: they keep the geometry and then the longest form of
/// the advice that still fits `marker_budget`, so a small budget shortens the
/// caller's sentence instead of throwing it away.
fn render_marker(cut: &Cut, advice: &str, detail: Detail, marker_budget: usize) -> Candidate {
    let total = cut.total_bytes;
    let dropped = cut.dropped_bytes;
    let whole = |text: String| Candidate {
        text,
        advice: Advice::Whole,
    };
    match detail {
        Detail::Full if cut.line_aligned => {
            let first = cut.first_dropped_line.unwrap_or(0);
            let last = first + cut.dropped_lines.saturating_sub(1);
            whole(format!(
                "\n\n[truncated: {total}-byte, {}-line result; result lines {first}-{last} \
                 ({dropped} bytes, {} lines) omitted from the middle; kept the first {} and the last \
                 {} bytes as whole lines. {advice}]\n\n",
                cut.total_lines, cut.dropped_lines, cut.head_bytes, cut.tail_bytes
            ))
        }
        Detail::Full => whole(format!(
            "\n\n[truncated: {total}-byte, {}-line result; {dropped} bytes omitted from the middle \
             (the cut falls inside a line); kept the first {} and the last {} bytes. {advice}]\n\n",
            cut.total_lines, cut.head_bytes, cut.tail_bytes
        )),
        Detail::Short if cut.line_aligned => {
            let first = cut.first_dropped_line.unwrap_or(0);
            let last = first + cut.dropped_lines.saturating_sub(1);
            compact_marker(
                &format!(
                    "[truncated: lines {first}-{last} ({dropped} bytes) omitted from the middle of \
                     this {total}-byte result;"
                ),
                advice,
                marker_budget,
            )
        }
        Detail::Short => compact_marker(
            &format!("[truncated: {dropped} of {total} bytes omitted from the middle;"),
            advice,
            marker_budget,
        ),
        Detail::Minimal => compact_marker(
            &format!("[truncated: {dropped} of {total} bytes omitted;"),
            advice,
            marker_budget,
        ),
    }
}

/// A compact marker: fixed geometry, then as much of the caller's advice as
/// `marker_budget` leaves room for.
///
/// The closing bracket and the blank lines around it are counted here because the
/// whole marker - not just its prose - is what has to fit. When not even the key
/// name fits, the marker goes out with its geometry alone; `assemble_cut` then
/// rejects it if even that is over budget and a smaller detail is tried.
fn compact_marker(prefix: &str, advice: &str, marker_budget: usize) -> Candidate {
    for (form, kept) in advice_forms(advice) {
        let text = format!("\n\n{prefix} {form}]\n\n");
        if text.len() <= marker_budget {
            return Candidate { text, advice: kept };
        }
    }
    // Nothing of the advice fits: drop the separator that introduced it rather
    // than leave a marker that trails off in a semicolon.
    Candidate {
        text: format!("\n\n{}]\n\n", prefix.trim_end_matches(';')),
        advice: Advice::None,
    }
}

/// The caller's advice at decreasing lengths, most of it first: the whole
/// sentence, its first clause, the sentence without its parenthetical aside, and
/// finally the key name it names last.
///
/// Shortening is what keeps the sentence rather than losing it: at a budget where
/// the whole clause does not fit, "re-run it as `cmd > out.txt 2>&1`" still tells
/// the model what this tool needs, and the key name alone is the smallest part an
/// operator can act on.
fn advice_forms(advice: &str) -> Vec<(String, Advice)> {
    let whole = advice.trim().trim_end_matches(['.', ';']).trim_end();
    let mut forms: Vec<(String, Advice)> = Vec::new();
    for form in [
        Some((whole.to_string(), Advice::Whole)),
        first_clause(whole).map(|clause| (clause, Advice::Short)),
        without_aside(whole).map(|bare| (bare, Advice::Short)),
        Some((format!("raise {TOOL_OUTPUT_LIMIT_KEY}"), Advice::Key)),
    ]
    .into_iter()
    .flatten()
    {
        if form.0.is_empty() || forms.iter().any(|kept| kept.0 == form.0) {
            continue;
        }
        forms.push(form);
    }
    forms
}

/// The sentence up to its first comma or semicolon that is outside parentheses
/// and backticks, when that leaves a clause long enough to act on.
fn first_clause(sentence: &str) -> Option<String> {
    let mut depth = 0i32;
    let mut in_code = false;
    for (i, ch) in sentence.char_indices() {
        match ch {
            '`' => in_code = !in_code,
            '(' if !in_code => depth += 1,
            ')' if !in_code => depth = (depth - 1).max(0),
            ',' | ';' if !in_code && depth == 0 => {
                let clause = sentence[..i].trim_end();
                return (clause.chars().count() >= MIN_ADVICE_CLAUSE_CHARS)
                    .then(|| clause.to_string());
            }
            _ => {}
        }
    }
    None
}

/// The sentence with its first parenthesised aside removed: "re-issue the call
/// with a narrower request (a smaller max_results, ...) or raise ..." keeps both
/// ends and drops the examples, which is exactly the part a tight budget can do
/// without.
fn without_aside(sentence: &str) -> Option<String> {
    let open = sentence.find('(')?;
    let close = open + sentence[open..].find(')')?;
    let mut bare = String::with_capacity(sentence.len());
    bare.push_str(sentence[..open].trim_end());
    let tail = sentence[close + 1..].trim_start();
    if !tail.is_empty() {
        if !bare.is_empty() {
            bare.push(' ');
        }
        bare.push_str(tail);
    }
    (bare.chars().count() >= MIN_ADVICE_CLAUSE_CHARS).then_some(bare)
}

/// A shortened clause below this is a fragment ("and", "or read") rather than an
/// instruction, so it is not offered as a form.
const MIN_ADVICE_CLAUSE_CHARS: usize = 12;

/// Largest char boundary `<= idx`.
fn floor_char(text: &str, idx: usize) -> usize {
    let mut end = idx.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// The end of the line at or before `idx` (itself a char boundary), when the
/// nearest newline is close enough to be a deliberate seam.
fn line_end_near(text: &str, idx: usize) -> Option<usize> {
    let idx = floor_char(text, idx);
    let window_start = idx.saturating_sub(MAX_LINE_SEAM_SHIFT);
    let pos = text.as_bytes()[window_start..idx]
        .iter()
        .rposition(|b| *b == b'\n')?;
    Some(window_start + pos + 1)
}

/// The start of the first whole line at or after `idx`.
fn line_start_near(text: &str, idx: usize) -> Option<usize> {
    let idx = floor_char(text, idx);
    if idx >= text.len() {
        return Some(text.len());
    }
    let window_end = (idx + MAX_LINE_SEAM_SHIFT).min(text.len());
    let pos = text.as_bytes()[idx..window_end]
        .iter()
        .position(|b| *b == b'\n')?;
    Some(idx + pos + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_respects_char_boundaries_and_the_final_cap() {
        let s = "héllo wörld";
        let (out, cut) = truncate_output(s, 4);
        assert!(cut);
        assert_eq!(out, "hél");
        assert!(out.len() <= 4);

        let long = format!("head-{}-TAIL-SENTINEL", "x".repeat(500));
        let (out, cut) = truncate_output(&long, 180);
        assert!(cut);
        assert!(out.len() <= 180, "{long:?} -> {}", out.len());
        assert!(out.starts_with("head-"), "{out}");
        assert!(out.ends_with("TAIL-SENTINEL"), "{out}");
        assert_eq!(out.matches("[truncated:").count(), 1, "{out}");
    }

    #[test]
    fn short_output_is_untouched() {
        let (out, cut) = truncate_output("abc", 10);
        assert!(!cut);
        assert_eq!(out, "abc");
    }

    /// A numbered, line-oriented payload of `n` lines.
    fn numbered(n: usize) -> String {
        (1..=n)
            .map(|i| format!("line {i}: {}", "x".repeat(20)))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    /// `(head, marker, tail)` split out of a cut result. The marker owns the
    /// blank lines that separate it from the content on both sides.
    fn split_marker(out: &str) -> (&str, &str, &str) {
        let start = out.find("[truncated:").expect("a cut carries a marker");
        let end = out[start..].find(']').map(|i| start + i + 1).expect("]");
        let before = &out[..start];
        let after = &out[end..];
        (
            before.strip_suffix("\n\n").unwrap_or(before),
            &out[start..end],
            after.strip_prefix("\n\n").unwrap_or(after),
        )
    }

    #[test]
    fn a_line_cut_reports_the_exact_line_range_it_dropped() {
        let text = numbered(2_000);
        let (out, cut) = truncate_output(&text, 1_200);
        assert!(cut);
        assert!(out.len() <= 1_200, "{} bytes", out.len());
        let (head, marker, tail) = split_marker(&out);
        assert!(marker.contains("omitted from the middle"), "{marker}");
        assert!(
            marker.contains(TOOL_OUTPUT_LIMIT_KEY),
            "the marker must name the key that bounds it: {marker}"
        );
        assert!(
            marker.contains("offset=/limit="),
            "the marker must name the call that continues it: {marker}"
        );
        // Never a mid-line fragment: the head is a prefix of the payload, the
        // tail is a suffix of it, and both seams sit on line boundaries.
        assert!(text.starts_with(head), "head is not a prefix: {head:?}");
        assert!(text.ends_with(tail), "tail is not a suffix: {tail:?}");
        assert!(head.ends_with('\n'), "head ends mid-line: {head:?}");
        assert!(
            tail.is_empty() || text[text.len() - tail.len() - 1..].starts_with('\n'),
            "tail starts mid-line: {tail:?}"
        );

        // The reported range really is the range that is missing.
        let head_lines = head.matches('\n').count();
        let dropped_lines = text.lines().count() - head_lines - tail.matches('\n').count();
        let first = head_lines + 1;
        let last = first + dropped_lines - 1;
        assert!(
            marker.contains(&format!("lines {first}-{last}")),
            "marker must report lines {first}-{last}: {marker}"
        );
        let payload: Vec<&str> = text.lines().collect();
        let tail_first = payload.len() - tail.matches('\n').count() + 1;
        assert_eq!(
            tail_first,
            last + 1,
            "the tail must resume exactly after the reported last dropped line"
        );
    }

    #[test]
    fn a_cut_is_deterministic() {
        let text = numbered(1_000);
        let a = truncate_output(&text, 900);
        let b = truncate_output(&text, 900);
        assert_eq!(a, b, "the same input must produce the same bytes");
    }

    #[test]
    fn a_tiny_budget_still_names_the_key_and_keeps_the_tail() {
        // One long line: there is no line boundary to land on, so the marker
        // reports bytes rather than pretending to know a line range.
        let text = format!("head-{}-TAIL-SENTINEL", "x".repeat(4_000));
        let (out, cut) = truncate_output(&text, 180);
        assert!(cut);
        assert!(out.len() <= 180, "{} bytes", out.len());
        assert!(out.ends_with("TAIL-SENTINEL"), "{out}");
        assert_eq!(out.matches("[truncated:").count(), 1, "{out}");
        assert!(
            out.contains(TOOL_OUTPUT_LIMIT_KEY),
            "even a compact marker names the key: {out}"
        );
        assert!(
            !out.contains("lines "),
            "a mid-line cut must not claim a line range: {out}"
        );
    }

    #[test]
    fn a_caller_can_supply_the_next_step_instead_of_the_generic_one() {
        let text = numbered(500);
        let advice = "re-run it as `cmd > /tmp/out.txt 2>&1` and read_file that file with offset=";
        let (out, cut) = truncate_output_with_advice(&text, 800, advice);
        assert!(cut);
        assert!(out.len() <= 800);
        assert!(out.contains(advice), "{out}");
        assert!(
            !out.contains("re-issue the call with a narrower request"),
            "the caller's advice replaces the generic sentence: {out}"
        );
    }

    /// The caller's sentence is the only part of a marker that tells the model
    /// what *this* tool needs, so a budget too small for all of it must shorten
    /// it rather than put the generic sentence back.
    #[test]
    fn a_compact_marker_keeps_the_callers_own_words() {
        let text = numbered(500);
        let advice = "re-run it as `cmd > /tmp/out.txt 2>&1`, then read_file it with offset=";

        // Room for the whole sentence: it survives verbatim.
        let (out, cut) = truncate_output_with_advice(&text, 400, advice);
        assert!(cut);
        assert!(out.len() <= 400, "{} bytes:\n{out}", out.len());
        assert!(
            out.contains(advice),
            "the caller's sentence was dropped:\n{out}"
        );

        // A budget with room for the first clause only: the clause - the part
        // that names the next call - survives, with the generic sentence still
        // gone.
        let (out, cut) = truncate_output_with_advice(&text, 180, advice);
        assert!(cut);
        assert!(out.len() <= 180, "{} bytes:\n{out}", out.len());
        assert!(
            !out.contains(advice),
            "the whole sentence cannot fit 180 bytes:\n{out}"
        );
        assert!(
            out.contains("cmd > /tmp/out.txt 2>&1"),
            "the shortened advice must keep the call the caller named:\n{out}"
        );
        assert!(
            !out.contains("re-issue the call with a narrower request"),
            "the generic sentence must not come back:\n{out}"
        );
    }

    /// A marker is carved out of the budget it reports, so it may never push the
    /// result past that budget - and it may only give up the caller's words when
    /// none of their shortened forms fits.
    #[test]
    fn a_compact_marker_never_outgrows_the_budget_it_is_carved_from() {
        let payloads = [numbered(500), format!("head-{}-TAIL", "x".repeat(4_000))];
        let advices = [
            TOOL_OUTPUT_LIMIT_ADVICE,
            "re-run it as `cmd > /tmp/out.txt 2>&1`, then read_file it with offset=",
        ];
        for text in &payloads {
            for advice in advices {
                for budget in [120usize, 160, 200, 260, 320, 400, 600, 900, 1_400, 2_400] {
                    let (out, cut) = truncate_output_with_advice(text, budget, advice);
                    assert!(cut);
                    assert!(
                        out.len() <= budget,
                        "a {budget}-byte budget produced {} bytes:\n{out}",
                        out.len()
                    );
                    let Some(start) = out.find("[truncated:") else {
                        assert!(
                            budget < 120,
                            "a {budget}-byte budget carried no marker:\n{out}"
                        );
                        continue;
                    };
                    let rest = &out[start..];
                    let end = rest
                        .find(']')
                        .unwrap_or_else(|| panic!("unclosed marker at {budget} bytes:\n{out}"));
                    let marker = &rest[..=end];
                    // The marker's share of the budget is the same one the cut
                    // carves it out of; the blank lines on both sides are part of
                    // it, as they are for `assemble_cut`.
                    let share = (budget / 2).max(80);
                    let marker_len = marker.len() + 4;
                    assert!(
                        marker_len <= share,
                        "the marker outgrew its {share}-byte share of a {budget}-byte budget \
                         ({marker_len} bytes):\n{marker}"
                    );
                    assert!(
                        !marker.contains(" ]") && !marker.contains(";]"),
                        "a shortened marker must not trail off in punctuation:\n{marker}"
                    );
                }
            }
        }
    }

    /// The same guarantee at the function that decides it: a compact marker is
    /// the longest form of the advice that fits the room it is given, and never
    /// one byte more - so a caller's sentence is only shortened when shortening
    /// it is what makes it fit.
    #[test]
    fn a_compact_marker_uses_the_longest_advice_form_that_fits() {
        let prefix = "[truncated: 900 of 1000 bytes omitted from the middle;";
        let advice = "re-run it as `cmd > /tmp/out.txt 2>&1`, then read_file it with offset=";
        let forms = advice_forms(advice);
        assert_eq!(
            forms.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>(),
            vec![
                advice,
                "re-run it as `cmd > /tmp/out.txt 2>&1`",
                "raise limits.max_tool_output_bytes",
            ],
            "the forms are the whole sentence, its first clause, then the key"
        );

        for budget in 60usize..=400 {
            let marker = compact_marker(prefix, advice, budget);
            assert!(
                marker.text.len() <= budget,
                "a {budget}-byte marker budget produced {} bytes:\n{}",
                marker.text.len(),
                marker.text
            );
            // The length a marker with each form would have, so the check is
            // "this one fits and that one does not", not a guess.
            let with = |form: &str| format!("\n\n{prefix} {form}]\n\n").len();
            let longest = forms.iter().find(|(form, _)| with(form) <= budget);
            match longest {
                Some((form, kept)) => {
                    assert!(
                        marker.text.contains(form.as_str()),
                        "`{form}` fits a {budget}-byte marker but was not used:\n{}",
                        marker.text
                    );
                    assert_eq!(marker.advice, *kept);
                }
                None => assert_eq!(marker.advice, Advice::None),
            }
        }
    }

    #[test]
    fn a_payload_without_newlines_is_reported_in_bytes() {
        let text = "y".repeat(10_000);
        let (out, cut) = truncate_output(&text, 600);
        assert!(cut);
        assert!(out.len() <= 600);
        let (_, marker, _) = split_marker(&out);
        assert!(marker.contains("bytes"), "{marker}");
        assert!(!marker.contains("lines 1-"), "{marker}");
    }

    /// A result that is cut again - a turn-budget squeeze, or a lowered key -
    /// must not end up with two markers around one seam, and must keep the tail
    /// the first cut kept. The context budget depends on this, and so does a
    /// reader: the older marker's numbers describe a splice that is no longer
    /// visible in the text.
    #[test]
    fn a_second_cut_does_not_stack_a_second_marker() {
        let text = numbered(400);
        let (once, cut) = truncate_output(&text, 1_600);
        assert!(cut);
        assert_eq!(once.matches("[truncated:").count(), 1);
        let kept_tail = &once[once.len() - 40..];

        let (twice, cut) = truncate_output(&once, 900);
        assert!(cut);
        assert!(twice.len() <= 900, "{} bytes", twice.len());
        assert_eq!(
            twice.matches("[truncated:").count(),
            1,
            "a re-cut stacks a second marker:\n{twice}"
        );
        assert!(
            twice.ends_with(kept_tail),
            "the tail the first cut kept must survive:\n{twice}"
        );

        // Same for a squeeze that keeps most of the body.
        let (thrice, cut) = truncate_output(&once, 1_400);
        assert!(cut);
        assert!(thrice.len() <= 1_400);
        assert_eq!(thrice.matches("[truncated:").count(), 1, "{thrice}");
        assert!(thrice.ends_with(kept_tail), "{thrice}");
    }

    #[test]
    fn a_cut_is_stable_across_repeated_runs() {
        let text = numbered(300);
        let first = truncate_output(&text, 1_000);
        for _ in 0..8 {
            assert_eq!(truncate_output(&text, 1_000), first);
        }
    }

    fn temp_file(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kc-core-capture-{tag}-{}",
            crate::ids::SessionId::new().as_str()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("file.bin");
        std::fs::write(&path, bytes).expect("write");
        path
    }

    #[test]
    fn a_missing_file_is_captured_as_absent() {
        let dir =
            std::env::temp_dir().join(format!("kc-core-capture-absent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let captured = capture_file(&dir.join("nope.txt"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(captured, CapturedFile::absent());
        assert!(!captured.existed, "the undo of a create removes the file");
        assert!(!captured.failed);
    }

    #[test]
    fn text_is_captured_verbatim() {
        let path = temp_file("text", b"fn main() {}\n");
        let captured = capture_file(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert_eq!(captured, CapturedFile::text("fn main() {}\n"));
    }

    #[test]
    fn a_binary_file_is_recorded_as_unrestorable_rather_than_mangled() {
        let path = temp_file("binary", &[0xff, 0xfe, 0x00, 0x01]);
        let captured = capture_file(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert!(
            captured.failed,
            "a lossy conversion would restore different bytes than it removed"
        );
        assert!(captured.existed);
        assert!(captured.content.is_empty());
    }

    #[test]
    fn a_file_over_the_cap_is_recorded_as_unrestorable() {
        let big = vec![b'a'; crate::checkpoint::MAX_SNAPSHOT_BYTES + 1];
        let path = temp_file("huge", &big);
        let captured = capture_file(&path);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert!(captured.failed, "half a file is worse than none");
    }

    #[test]
    fn registry_orders_and_replaces() {
        struct Dummy(ToolSpec);
        #[async_trait]
        impl Tool for Dummy {
            fn spec(&self) -> ToolSpec {
                self.0.clone()
            }
            async fn invoke(&self, _r: ToolCallRequest, _c: ToolCtx) -> ToolOutcome {
                ToolOutcome::ok("")
            }
        }
        let mk = |n: &str| -> Arc<dyn Tool> {
            Arc::new(Dummy(ToolSpec {
                name: n.into(),
                description: String::new(),
                parameters: serde_json::json!({}),
                origin: ToolOrigin::Builtin,
            }))
        };
        let mut reg = ToolRegistry::new();
        reg.register(mk("b"));
        reg.register(mk("a"));
        assert_eq!(reg.names(), vec!["a".to_string(), "b".to_string()]);
        reg.register(mk("a"));
        assert_eq!(reg.len(), 2);
    }

    fn read_file_spec() -> ToolSpec {
        ToolSpec {
            name: "read_file".into(),
            description: "Read a file.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "offset": {"type": "integer"},
                    "limit": {"type": "integer"}
                },
                "required": ["path"]
            }),
            origin: ToolOrigin::Builtin,
        }
    }

    #[test]
    fn the_example_call_fills_every_required_argument() {
        let spec = read_file_spec();
        let example = spec.example_arguments();
        assert_eq!(example["path"], "<string>");
        assert!(
            example.get("offset").is_none(),
            "optional arguments must not be invented: {example}"
        );
        let call = spec.example_call();
        assert!(call.contains("\"name\":\"read_file\""), "{call}");
        assert!(call.contains("\"path\""), "{call}");
    }

    #[test]
    fn the_example_prefers_values_the_schema_itself_offers() {
        let spec = ToolSpec {
            name: "karucode_docs".into(),
            description: String::new(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["list", "search", "read"]},
                    "limit": {"type": "integer", "default": 5},
                    "path": {"type": "string", "default": "."}
                },
                "required": ["action"]
            }),
            origin: ToolOrigin::Builtin,
        };
        assert_eq!(spec.example_arguments()["action"], "list");
        let with_default = ToolSpec {
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string", "default": "."}},
            }),
            ..spec
        };
        // No required list: the first declared property is still made concrete.
        assert_eq!(with_default.example_arguments()["path"], ".");
    }

    #[test]
    fn a_tool_with_no_arguments_gets_no_hint() {
        let spec = ToolSpec {
            name: "git_status".into(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            origin: ToolOrigin::Builtin,
        };
        assert_eq!(spec.failure_hint(), "");
    }

    #[test]
    fn the_failure_hint_is_bounded_and_keeps_the_call_intact() {
        let mut props = serde_json::Map::new();
        for i in 0..40 {
            props.insert(
                format!("argument_number_{i}"),
                serde_json::json!({"type": "string", "description": "x".repeat(200)}),
            );
        }
        let spec = ToolSpec {
            name: "big".into(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object", "properties": props}),
            origin: ToolOrigin::Builtin,
        };
        let hint = spec.failure_hint();
        assert!(hint.len() <= FAILURE_HINT_MAX_BYTES, "{}", hint.len());
        assert!(hint.contains("\"name\":\"big\""), "{hint}");
        assert!(hint.contains("argument_number_0"), "{hint}");
        assert!(hint.ends_with("\"]") || hint.contains("...]"), "{hint}");
    }

    #[test]
    fn arguments_are_checked_against_the_schema_they_were_offered() {
        let spec = read_file_spec();
        assert!(spec
            .validate_arguments(&serde_json::json!({"path": "a.txt"}))
            .is_ok());

        let problems = spec
            .validate_arguments(&serde_json::json!({"path": null}))
            .unwrap_err();
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("`path` must be string")
                    && problem.contains("got null")),
            "a required non-nullable string rejects null: {problems:?}"
        );

        let problems = spec
            .validate_arguments(&serde_json::json!({"offset": 3}))
            .unwrap_err();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("missing required argument `path`"));

        let problems = spec
            .validate_arguments(&serde_json::json!({"path": "a", "offset": "3"}))
            .unwrap_err();
        assert!(
            problems[0].contains("`offset` must be integer"),
            "{problems:?}"
        );

        // A top-level non-object is a problem of its own.
        let problems = spec
            .validate_arguments(&serde_json::json!("a.txt"))
            .unwrap_err();
        assert!(problems[0].contains("takes a JSON object, got string"));
    }

    #[test]
    fn validation_respects_enum_nullable_and_additional_properties() {
        let spec = ToolSpec {
            name: "t".into(),
            description: String::new(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["list", "read"]},
                    "base": {"type": ["string", "null"]},
                    "choice": {"enum": ["auto", null]},
                    "limit": {"type": "number"}
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            origin: ToolOrigin::Builtin,
        };
        assert!(spec
            .validate_arguments(&serde_json::json!({"action": "read", "base": null}))
            .is_ok());
        assert!(spec
            .validate_arguments(&serde_json::json!({"action": "read", "choice": null}))
            .is_ok());
        let problems = spec
            .validate_arguments(&serde_json::json!({"action": null}))
            .unwrap_err();
        assert!(
            problems.iter().any(|problem| {
                problem.contains("must be one of") && problem.contains("got null")
            }),
            "an enum without null rejects null: {problems:?}"
        );
        // A whole integer is a valid `number`.
        assert!(spec
            .validate_arguments(&serde_json::json!({"action": "list", "limit": 2}))
            .is_ok());

        let problems = spec
            .validate_arguments(&serde_json::json!({"action": "dance", "typo": 1}))
            .unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("must be one of")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|p| p.contains("unknown argument `typo`")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_schema_without_types_is_not_second_guessed() {
        // An MCP server may publish a bare `{"type":"object"}`; refusing every
        // argument it did not describe would break a call the server accepts.
        let spec = ToolSpec {
            name: "mcp__fs__read".into(),
            description: String::new(),
            parameters: serde_json::json!({"type": "object"}),
            origin: ToolOrigin::Mcp {
                server: "fs".into(),
            },
        };
        assert!(spec
            .validate_arguments(&serde_json::json!({"anything": [1, 2]}))
            .is_ok());
        assert_eq!(spec.failure_hint(), "");
    }

    #[test]
    fn malformed_arguments_are_repaired_into_the_call_that_was_meant() {
        let cases = [
            // Fenced, with prose around it.
            (
                "```json\n{\"path\": \"a.txt\"}\n```",
                "{\"path\":\"a.txt\"}",
            ),
            // A sentence first.
            (
                "Call this: {\"path\": \"a.txt\"} please",
                "{\"path\":\"a.txt\"}",
            ),
            // Trailing comma.
            ("{\"path\": \"a.txt\",}", "{\"path\":\"a.txt\"}"),
            // Single quotes.
            ("{'path': 'a.txt'}", "{\"path\":\"a.txt\"}"),
            // Both at once.
            ("{'path': 'a.txt', }", "{\"path\":\"a.txt\"}"),
        ];
        for (raw, expected) in cases {
            let repaired =
                repair_arguments(raw).unwrap_or_else(|| panic!("could not repair {raw:?}"));
            assert_eq!(repaired.to_string(), expected, "input was {raw:?}");
        }
    }

    #[test]
    fn repair_reports_nothing_when_nothing_parses() {
        assert!(repair_arguments("{not json at all").is_none());
        assert!(repair_arguments("").is_none());
        assert!(repair_arguments("no braces here").is_none());
    }

    #[test]
    fn repair_unwraps_a_double_encoded_object() {
        let repaired = repair_arguments("\"{\\\"path\\\":\\\"a.txt\\\"}\"").unwrap();
        assert_eq!(repaired["path"], "a.txt");
    }

    #[test]
    fn repair_does_not_mangle_an_apostrophe_inside_a_json_string() {
        let repaired =
            repair_arguments("{\"old_string\": \"it's fine\", \"new_string\": \"ok\"}").unwrap();
        assert_eq!(repaired["old_string"], "it's fine");
    }

    #[test]
    fn a_typo_in_a_tool_name_is_suggested_by_closeness() {
        let names = ["read_file", "write_file", "run_command", "search_files"];
        assert_eq!(
            closest_tool_name("read_files", names).as_deref(),
            Some("read_file")
        );
        assert_eq!(
            closest_tool_name("Read_File", names).as_deref(),
            Some("read_file")
        );
        assert_eq!(
            closest_tool_name("searchfiles", names).as_deref(),
            Some("search_files")
        );
        // An unrelated name must not be "corrected" into a different tool.
        assert_eq!(closest_tool_name("spawn_agent", names), None);
        assert_eq!(closest_tool_name("", names), None);
    }
}
