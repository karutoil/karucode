//! Context budgeting: truncate tool output and drop the oldest exchanges while
//! keeping the system prompt and the latest user turn.
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
//! After trimming, a single assistant notice reports how many messages were
//! dropped.

use kc_core::message::{Message, Role};
use kc_core::tool::truncate_output;

const TRIM_PREFIX: &str = "[earlier conversation trimmed: ";

/// Number of characters a message contributes to the budget.
fn message_chars(message: &Message) -> usize {
    let calls: usize = message
        .tool_calls
        .iter()
        .map(|c| c.name.chars().count() + c.arguments.chars().count())
        .sum();
    message.content.chars().count() + calls
}

/// Drop oldest exchanges until the budget fits, then insert the trim notice.
///
/// `messages` must be ordered oldest-first and is expected to start with the
/// system prompt. Returns an unchanged copy when everything already fits.
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
    let mut total = total;
    let mut dropped_messages = 0usize;
    let mut dropped = vec![false; messages.len()];
    for unit in &units {
        if total <= max_chars {
            break;
        }
        let unit_chars: usize = unit.iter().map(|&i| message_chars(&messages[i])).sum();
        total = total.saturating_sub(unit_chars);
        dropped_messages += unit.len();
        for &i in unit {
            dropped[i] = true;
        }
    }

    if dropped_messages == 0 {
        return messages;
    }

    // Rebuild: system messages keep their position, then the notice, then the
    // survivors of the non-system region.
    let mut out: Vec<Message> = Vec::with_capacity(messages.len() - dropped_messages + 1);
    let mut notice_inserted = false;
    for (i, message) in messages.into_iter().enumerate() {
        if dropped[i] {
            continue;
        }
        if !notice_inserted && message.role != Role::System {
            out.push(Message::assistant(
                format!("{TRIM_PREFIX}{dropped_messages} messages]"),
                Vec::new(),
            ));
            notice_inserted = true;
        }
        out.push(message);
    }
    if !notice_inserted {
        out.push(Message::assistant(
            format!("{TRIM_PREFIX}{dropped_messages} messages]"),
            Vec::new(),
        ));
    }
    out
}

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
        assert!(notices[0].content.ends_with("messages]"));
        // The notice sits directly after the system prompt.
        assert_eq!(out[1].content, notices[0].content);
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
