//! Context budgeting: truncate tool output and compact the oldest exchanges
//! while keeping the system prompt and the latest user turn.
//!
//! Two independent steps, both applied to the message list actually sent to the
//! model (the persisted transcript is never rewritten):
//!
//! 1. every `tool` message content is capped at `max_tool_output_bytes`;
//! 2. while the total character budget is exceeded, the oldest droppable
//!    exchange is removed. An assistant message and the tool results that answer
//!    its `tool_calls` are dropped as one group, so the model never sees a tool
//!    result whose call is missing. The system messages and the final user turn
//!    are never removed.
//!
//! Dropped turns are replaced by one compaction notice: a digest of the trimmed
//! user/assistant turns (and the tools they called), bounded by
//! [`DIGEST_MAX_CHARS`] and counted inside the same budget. A long run therefore
//! keeps the gist of its early work instead of going blind after the first trim.
//! The notice is placed directly after the system prompt.

use kc_core::message::{Message, Role};
use kc_core::tool::truncate_output;

const TRIM_PREFIX: &str = "[compacted earlier conversation: ";

/// How much space the compaction digest may take. It is a summary, not a
/// transcript: if it grew without a bound it would defeat the budget it exists
/// to protect.
const DIGEST_MAX_CHARS: usize = 4_000;
/// Characters of one dropped message kept in the digest.
const DIGEST_LINE_CHARS: usize = 160;

/// Number of characters a message contributes to the budget.
fn message_chars(message: &Message) -> usize {
    let calls: usize = message
        .tool_calls
        .iter()
        .map(|c| c.name.chars().count() + c.arguments.chars().count())
        .sum();
    message.content.chars().count() + calls
}

/// Drop oldest exchanges until the budget fits, then insert the compaction
/// notice.
///
/// `messages` must be ordered oldest-first and is expected to start with the
/// system prompt. Returns an unchanged copy when everything already fits.
///
/// The notice costs characters, so it is part of the budget: when the digest no
/// longer fits, one more exchange is dropped and the digest is rebuilt. The
/// result therefore always respects `max_chars` (unless a single protected
/// message is larger than the whole budget, which the model cannot avoid).
pub fn fit(messages: Vec<Message>, max_chars: usize, max_tool_output_bytes: usize) -> Vec<Message> {
    let messages: Vec<Message> = messages
        .into_iter()
        .map(|mut m| {
            if m.role == Role::Tool {
                let (content, _) = truncate_output(&m.content, max_tool_output_bytes);
                m.content = content;
            }
            m
        })
        .collect();

    let total: usize = messages.iter().map(message_chars).sum();
    if total <= max_chars {
        return messages;
    }

    // Everything from the last user turn onwards is protected.
    let protected_from = messages
        .iter()
        .rposition(|m| m.role == Role::User)
        .unwrap_or(0);

    let units = group_droppable(&messages[..protected_from]);
    let mut survivors = total;
    let mut dropped_messages = 0usize;
    let mut dropped = vec![false; messages.len()];
    let mut next = 0usize;

    let notice = loop {
        // 1. Make the surviving messages fit.
        while survivors > max_chars && next < units.len() {
            let unit = &units[next];
            let unit_chars: usize = unit.iter().map(|&i| message_chars(&messages[i])).sum();
            survivors = survivors.saturating_sub(unit_chars);
            dropped_messages += unit.len();
            for &i in unit {
                dropped[i] = true;
            }
            next += 1;
        }
        if dropped_messages == 0 {
            return messages;
        }
        // 2. The digest has to fit too, or the summary would defeat the budget.
        let notice = compact_notice(&messages, &dropped, dropped_messages, max_chars);
        if survivors + notice.chars().count() <= max_chars || next >= units.len() {
            break notice;
        }
        // 3. No room for the digest: drop one more exchange and rebuild it.
        let unit = &units[next];
        let unit_chars: usize = unit.iter().map(|&i| message_chars(&messages[i])).sum();
        survivors = survivors.saturating_sub(unit_chars);
        dropped_messages += unit.len();
        for &i in unit {
            dropped[i] = true;
        }
        next += 1;
    };

    // Rebuild: system messages keep their position, then the notice, then the
    // survivors of the non-system region.
    let mut out: Vec<Message> = Vec::with_capacity(messages.len() - dropped_messages + 1);
    let mut notice_inserted = false;
    for (i, message) in messages.into_iter().enumerate() {
        if dropped[i] {
            continue;
        }
        if !notice_inserted && message.role != Role::System {
            out.push(Message::assistant(notice.clone(), Vec::new()));
            notice_inserted = true;
        }
        out.push(message);
    }
    if !notice_inserted {
        out.push(Message::assistant(notice, Vec::new()));
    }
    out
}

/// What the model gets instead of the turns that were dropped.
///
/// A bare "N messages trimmed" leaves a long run blind to everything it learned
/// early on. The digest keeps one trimmed line per dropped user/assistant turn
/// (with a caret list of the tools it called) and counts the tool results, so a
/// later segment can still remember what was decided and which files were
/// touched. Newest dropped turns win when the digest budget runs out.
fn compact_notice(
    messages: &[Message],
    dropped: &[bool],
    dropped_messages: usize,
    max_chars: usize,
) -> String {
    let (digest_chars, line_chars) = digest_caps(max_chars);
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut tool_results = 0usize;
    for (i, message) in messages.iter().enumerate().rev() {
        if !dropped[i] {
            continue;
        }
        let line = match message.role {
            Role::Tool => {
                tool_results += 1;
                continue;
            }
            Role::System => continue,
            Role::User => format!("- user: {}", one_line(&message.content, line_chars)),
            Role::Assistant => {
                let mut text = one_line(&message.content, line_chars);
                if !message.tool_calls.is_empty() {
                    let names: Vec<&str> = message
                        .tool_calls
                        .iter()
                        .map(|call| call.name.as_str())
                        .collect();
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&format!("[called {}]", names.join(", ")));
                }
                format!("- assistant: {text}")
            }
        };
        let chars = line.chars().count() + 1;
        if used + chars > digest_chars {
            break;
        }
        used += chars;
        lines.push(line);
    }
    lines.reverse();

    let mut notice = format!("{TRIM_PREFIX}{dropped_messages} messages omitted");
    if tool_results > 0 {
        notice.push_str(&format!(", {tool_results} tool results"));
    }
    if lines.is_empty() {
        notice.push(']');
        return notice;
    }
    notice.push_str("; digest:]\n");
    for line in lines {
        notice.push_str(&line);
        notice.push('\n');
    }
    notice.trim_end().to_string()
}

/// Size caps for the digest, scaled to the budget it lives in.
///
/// A fixed 4k digest would be harmless in a 240k budget and absurd in a 2k one,
/// so the digest may take at most an eighth of the budget and each line at most a
/// fortieth, with a floor that still leaves a recognizable fragment.
fn digest_caps(max_chars: usize) -> (usize, usize) {
    let total = DIGEST_MAX_CHARS.min((max_chars / 8).max(200));
    let line = DIGEST_LINE_CHARS.min((max_chars / 40).max(24));
    (total, line)
}

/// Collapse a message body to one digest line: newlines flattened, whitespace
/// squeezed, and a hard character cut (never a byte cut, so UTF-8 survives).
fn one_line(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut last_space = false;
    for ch in text.chars() {
        if out.chars().count() >= max_chars {
            out.push('\u{2026}');
            break;
        }
        if ch.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
                last_space = true;
            }
            continue;
        }
        out.push(ch);
        last_space = false;
    }
    out.trim().to_string()
}

/// Keep the **in-memory** history of a running loop inside the request budget.
///
/// `fit` bounds what is *sent*; this bounds what is *held*. Without it a session
/// with thousands of tool calls keeps every result in memory and the loop clones
/// the whole conversation into the request on every iteration — cost that grows
/// with the length of the session and is pure waste, because anything past the
/// budget can never be sent anyway.
///
/// The newest exchanges are protected (the model is working on them right now),
/// whole exchanges are dropped together, and the dropped turns are digested into
/// one notice at the front so the model keeps the gist of the earlier work.
pub fn compact_run_window(history: &mut Vec<Message>, max_chars: usize) {
    let total: usize = history.iter().map(message_chars).sum();
    if total <= max_chars || history.len() <= PROTECTED_TAIL {
        return;
    }

    let protected_from = history.len().saturating_sub(PROTECTED_TAIL);
    let units = group_droppable(history);
    let mut dropped = vec![false; history.len()];
    let mut dropped_messages = 0usize;
    let mut survivors = total;
    let mut next = 0usize;

    // The digest costs characters too, so it is part of the budget: keep
    // dropping (whole exchanges) until survivors + notice fit.
    let notice = loop {
        while survivors > max_chars && next < units.len() {
            if units[next].iter().any(|&i| i >= protected_from) {
                break;
            }
            let unit = &units[next];
            let unit_chars: usize = unit.iter().map(|&i| message_chars(&history[i])).sum();
            survivors = survivors.saturating_sub(unit_chars);
            dropped_messages += unit.len();
            for &i in unit {
                dropped[i] = true;
            }
            next += 1;
        }
        if dropped_messages == 0 {
            return;
        }
        let notice = compact_notice(history, &dropped, dropped_messages, max_chars);
        let fits = survivors + notice.chars().count() <= max_chars;
        let can_drop_more = next < units.len() && !units[next].iter().any(|&i| i >= protected_from);
        if fits || !can_drop_more {
            break notice;
        }
        let unit = &units[next];
        let unit_chars: usize = unit.iter().map(|&i| message_chars(&history[i])).sum();
        survivors = survivors.saturating_sub(unit_chars);
        dropped_messages += unit.len();
        for &i in unit {
            dropped[i] = true;
        }
        next += 1;
    };

    let mut out: Vec<Message> = Vec::with_capacity(history.len() - dropped_messages + 1);
    out.push(Message::assistant(notice, Vec::new()));
    for (i, message) in history.drain(..).enumerate() {
        if !dropped[i] {
            out.push(message);
        }
    }
    *history = out;
}

/// How many of the newest messages a run window never drops, so the model always
/// sees the exchange it is in the middle of.
const PROTECTED_TAIL: usize = 32;

/// Split the droppable prefix into units that must be removed together:
/// an assistant message together with the tool results that answer it, or a
/// single message otherwise. System messages are never part of a unit.
fn group_droppable(prefix: &[Message]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut i = 0usize;
    while i < prefix.len() {
        if prefix[i].role == Role::System {
            i += 1;
            continue;
        }
        let mut unit = vec![i];
        let is_call = prefix[i].role == Role::Assistant && !prefix[i].tool_calls.is_empty();
        let mut j = i + 1;
        if is_call {
            while j < prefix.len() && prefix[j].role == Role::Tool {
                unit.push(j);
                j += 1;
            }
        } else if prefix[i].role == Role::Tool {
            // Orphan tool result (e.g. from an old cancelled run): keep it with
            // the following tool results so it is never alone.
            while j < prefix.len() && prefix[j].role == Role::Tool {
                unit.push(j);
                j += 1;
            }
        }
        groups.push(unit);
        i = j;
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::message::ToolCall;

    fn tool_msg(id: &str, body: &str) -> Message {
        Message::tool_result(id, "echo", body)
    }

    fn call_msg(id: &str) -> Message {
        Message::assistant(
            "",
            vec![ToolCall {
                id: id.into(),
                name: "echo".into(),
                arguments: "{}".into(),
            }],
        )
    }

    #[test]
    fn small_conversations_are_untouched() {
        let msgs = vec![
            Message::system("sys"),
            Message::user("hello"),
            Message::assistant("hi", vec![]),
        ];
        let out = fit(msgs.clone(), 10_000, 10_000);
        assert_eq!(out.len(), msgs.len());
        assert_eq!(out[0].content, "sys");
    }

    #[test]
    fn tool_output_is_truncated_to_the_byte_budget() {
        let big = "x".repeat(4000);
        let out = fit(
            vec![
                Message::system("sys"),
                Message::user("u"),
                tool_msg("c1", &big),
            ],
            1_000_000,
            500,
        );
        let tool = out.iter().find(|m| m.role == Role::Tool).unwrap();
        assert!(tool.content.len() < 1000);
        assert!(tool.content.contains("bytes omitted"));
    }

    #[test]
    fn trimming_keeps_system_and_last_user_turn() {
        let mut msgs = vec![Message::system("SYSTEM PROMPT")];
        for i in 0..20 {
            msgs.push(Message::user(format!(
                "old user {i} with some padding text"
            )));
            msgs.push(call_msg(&format!("c{i}")));
            msgs.push(tool_msg(&format!("c{i}"), &"y".repeat(500)));
            msgs.push(Message::assistant("ok", vec![]));
        }
        msgs.push(Message::user("CURRENT QUESTION"));
        let out = fit(msgs, 4000, 100_000);
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[0].content, "SYSTEM PROMPT");
        let last = out.last().unwrap();
        assert_eq!(last.role, Role::User);
        assert_eq!(last.content, "CURRENT QUESTION");
        let total: usize = out.iter().map(message_chars).sum();
        assert!(total <= 4000);
    }

    #[test]
    fn trimming_never_orphans_tool_messages() {
        let mut msgs = vec![Message::system("sys"), Message::user("first")];
        for i in 0..10 {
            msgs.push(call_msg(&format!("c{i}")));
            msgs.push(tool_msg(&format!("c{i}"), &"z".repeat(800)));
            msgs.push(Message::assistant("next", vec![]));
        }
        msgs.push(Message::user("last"));
        let out = fit(msgs, 3000, 100_000);
        // Every tool result must be preceded by an assistant tool call with the
        // same id somewhere earlier in the list.
        let mut seen_calls = std::collections::HashSet::new();
        for m in &out {
            for c in &m.tool_calls {
                seen_calls.insert(c.id.clone());
            }
            if m.role == Role::Tool {
                let id = m.tool_call_id.clone().unwrap();
                assert!(seen_calls.contains(&id), "orphaned tool result {id}");
            }
        }
        assert!(out.last().unwrap().content == "last");
    }

    #[test]
    fn a_single_notice_reports_how_many_messages_were_dropped() {
        let mut msgs = vec![Message::system("sys"), Message::user("first")];
        for i in 0..6 {
            msgs.push(call_msg(&format!("c{i}")));
            msgs.push(tool_msg(&format!("c{i}"), &"q".repeat(600)));
        }
        msgs.push(Message::user("last"));
        let out = fit(msgs, 2000, 100_000);
        let notices: Vec<&Message> = out
            .iter()
            .filter(|m| m.content.starts_with(TRIM_PREFIX))
            .collect();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].role, Role::Assistant);
        assert!(notices[0].content.contains("messages omitted"));
        // The notice sits directly after the system prompt.
        assert_eq!(out[1].content, notices[0].content);
    }

    #[test]
    fn the_notice_carries_a_digest_of_what_was_dropped() {
        let mut msgs = vec![Message::system("sys")];
        for i in 0..8 {
            msgs.push(Message::user(format!(
                "step {i}: rewrote the parser entry point"
            )));
            msgs.push(call_msg(&format!("c{i}")));
            msgs.push(tool_msg(&format!("c{i}"), &"z".repeat(400)));
            msgs.push(Message::assistant("done", vec![]));
        }
        msgs.push(Message::user("continue"));
        let out = fit(msgs, 2500, 100_000);
        let notice = out
            .iter()
            .find(|m| m.content.starts_with(TRIM_PREFIX))
            .expect("compaction notice");
        assert!(
            notice.content.contains("step 1"),
            "the newest dropped turn must survive in the digest: {}",
            notice.content
        );
        assert!(notice.content.contains("[called echo]"));
        // The digest is summarised, never a copy of the transcript.
        assert!(notice.content.chars().count() <= DIGEST_MAX_CHARS + 200);
        // And it is counted inside the budget, not added on top of it.
        let total: usize = out.iter().map(message_chars).sum();
        assert!(total <= 2500, "budget respected: {total}");
    }

    #[test]
    fn unbudgeted_system_prompt_is_never_dropped_even_if_huge() {
        let msgs = vec![Message::system("s".repeat(5000)), Message::user("hello")];
        let out = fit(msgs, 100, 100_000);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].role, Role::System);
        assert_eq!(out[1].content, "hello");
    }
}

#[cfg(test)]
mod run_window_tests {
    use super::*;
    use kc_core::message::ToolCall;

    fn exchange(i: usize, body: usize) -> Vec<Message> {
        vec![
            Message::user(format!("step {i}: inspect the parser")),
            Message::assistant(
                "",
                vec![ToolCall {
                    id: format!("c{i}"),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                }],
            ),
            Message::tool_result(
                format!("c{i}"),
                "read_file",
                format!("{i}:{}", "x".repeat(body)),
            ),
            Message::assistant("done", vec![]),
        ]
    }

    #[test]
    fn a_long_run_window_stays_inside_the_budget() {
        let mut history = Vec::new();
        for i in 0..300 {
            history.extend(exchange(i, 2_000));
        }
        let before: usize = history.iter().map(message_chars).sum();
        assert!(before > 600_000, "fixture should be far over budget");

        compact_run_window(&mut history, 120_000);

        let after: usize = history.iter().map(message_chars).sum();
        assert!(
            after <= 120_000,
            "window must respect the budget: {after} chars"
        );
        assert!(
            after * 4 < before,
            "window should shrink hard: {before} -> {after}"
        );
        // The newest exchange is still there: the model keeps the turn it is in.
        assert!(history.iter().any(|m| m.content.contains("step 299")));
        // Nothing in the window may be an orphaned tool result.
        let mut seen = std::collections::HashSet::new();
        for m in &history {
            for c in &m.tool_calls {
                seen.insert(c.id.clone());
            }
            if m.role == Role::Tool {
                let id = m.tool_call_id.clone().unwrap();
                assert!(
                    seen.contains(&id),
                    "orphaned tool result {id} at the front of the window"
                );
            }
        }
        // The dropped turns left a digest behind.
        let notice = history
            .first()
            .expect("window is not empty")
            .content
            .clone();
        assert!(notice.starts_with(TRIM_PREFIX), "{notice}");
    }

    #[test]
    fn a_short_run_window_is_untouched() {
        let mut history = Vec::new();
        for i in 0..3 {
            history.extend(exchange(i, 10));
        }
        let before = history.clone();
        compact_run_window(&mut history, 120_000);
        assert_eq!(history.len(), before.len());
    }

    #[test]
    fn repeated_compaction_keeps_the_rolling_summary_bounded() {
        let mut history = Vec::new();
        for round in 0..20 {
            for i in 0..50 {
                history.extend(exchange(round * 50 + i, 1_000));
            }
            compact_run_window(&mut history, 40_000);
            let total: usize = history.iter().map(message_chars).sum();
            assert!(
                total <= 40_000,
                "round {round}: window grew to {total} chars"
            );
        }
    }
}
