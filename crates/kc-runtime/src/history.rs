//! Transcript repair for stored conversations.
//!
//! OpenAI-compatible endpoints reject a request whose tool results are not
//! attached to the assistant turn that requested them. Two things can produce
//! such a transcript:
//!
//! * the history window ([`crate::HISTORY_LIMIT`]) can start in the middle of a
//!   tool exchange, leaving a `tool` row whose `assistant` call is outside the
//!   window;
//! * a store failure (or panic) while persisting a tool result leaves the
//!   assistant message with `tool_calls` and no matching `tool` row.
//!
//! [`repair_history`] normalises the stored rows before every run and during
//! startup recovery: orphan tool results are dropped, missing results are
//! synthesized (and persisted), and every surviving tool result is placed
//! directly after the assistant turn that asked for it. A previously persisted
//! synthesized row is reused on later loads, so the repair is idempotent and
//! does not grow the transcript.

use kc_core::error::Result;
use kc_core::message::{Message, Role, ToolCall};
use kc_core::store::{NewMessage, Store, StoredMessage};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Content of a synthesized tool result. Also used to recognise rows created by
/// an earlier repair, so they are reused instead of appended again.
pub(crate) const MISSING_TOOL_RESULT: &str =
    "[tool result missing: the run was interrupted before this call returned]";

/// A tool result the repair had to invent, plus the run it belongs to.
struct Synthesized {
    run_id: Option<String>,
    call: ToolCall,
}

/// Load, repair and (if needed) persist the conversation for `session_id`.
///
/// Returns the message list in model-ready order: every `tool` message follows
/// the assistant message that declared its `tool_call_id`. The final user
/// message is never dropped; subagent transcripts and stored system prompts are
/// excluded, because the caller rebuilds the system prompt for every request.
pub(crate) async fn repair_history(
    store: &Arc<dyn Store>,
    session_id: &str,
    limit: usize,
    max_chars: usize,
) -> Result<Vec<Message>> {
    let stored = store.list_messages(session_id, limit).await?;
    let stored: Vec<StoredMessage> = stored
        .into_iter()
        // Subagent transcripts share the session but must never leak into the
        // parent conversation; stored system prompts are rebuilt per iteration.
        .filter(|m| m.agent_id.is_none() && m.role != Role::System.as_str())
        .collect();

    let (mut messages, synthesized) = repair_rows(&stored);

    // A count-only window is not a memory bound: 20_000 messages of 24 KB tool
    // output is half a gigabyte. Keep the newest exchanges that fit the budget
    // the request will use, so loading a month-old session costs the same as a
    // fresh one.
    trim_front_to_chars(&mut messages, max_chars);

    // Persist the repair so a restart does not resurrect the broken transcript.
    // The store is append-only, so a missing result that is not at the tail is
    // written at the end and relocated into place by the next `repair_rows`.
    for missing in synthesized {
        let record = NewMessage {
            session_id: session_id.to_string(),
            run_id: missing.run_id,
            agent_id: None,
            role: Role::Tool.as_str().to_string(),
            content: MISSING_TOOL_RESULT.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some(missing.call.id),
            name: Some(missing.call.name),
        };
        store.append_message(&record).await?;
    }

    Ok(messages)
}

/// Drop the oldest messages until the slice fits `max_chars`.
///
/// The cut is moved forward to a message that is not a `tool` result, so the
/// window never *starts* with an orphaned result; the exchange that owned it is
/// dropped as a whole (the repair above guarantees call-then-results order). The
/// newest message is always kept, so a single oversized result cannot empty the
/// window.
pub(crate) fn trim_front_to_chars(messages: &mut Vec<Message>, max_chars: usize) {
    let chars = |m: &Message| m.content.chars().count();
    let mut total: usize = messages.iter().map(chars).sum();
    let mut start = 0usize;
    while total > max_chars && start + 1 < messages.len() {
        total = total.saturating_sub(chars(&messages[start]));
        start += 1;
    }
    // Never begin the window with a tool result whose call was dropped.
    while start + 1 < messages.len() && messages[start].role == Role::Tool {
        start += 1;
    }
    if start > 0 {
        messages.drain(..start);
    }
}

/// Structural repair of an ordered (oldest-first) transcript slice.
fn repair_rows(stored: &[StoredMessage]) -> (Vec<Message>, Vec<Synthesized>) {
    // Every tool result grouped by call id, oldest first. A call id can repeat
    // across turns (`call_0` from a cheap gateway, or the same id in a replayed
    // request), so a result is matched to the first *unused* row that comes after
    // the call that asked for it - never to a single global "the" row, which made
    // every repeat synthesize a fresh placeholder and grow the transcript by one
    // row per turn, forever (the store is append-only).
    let mut results: HashMap<&str, Vec<&StoredMessage>> = HashMap::new();
    for row in stored {
        if row.role != Role::Tool.as_str() {
            continue;
        }
        let Some(id) = row.tool_call_id.as_deref() else {
            continue; // a tool row without an id cannot be attached to anything
        };
        results.entry(id).or_default().push(row);
    }

    let mut used: HashSet<i64> = HashSet::new();
    let mut messages = Vec::with_capacity(stored.len());
    let mut synthesized = Vec::new();
    for row in stored {
        match row.role.as_str() {
            "assistant" => {
                let message = row.to_message();
                let calls = message.tool_calls.clone();
                messages.push(message);
                // Emit the results of this turn, in the order the model asked
                // for them. Anything that does not belong to this assistant is
                // skipped when its own row is reached.
                for call in calls {
                    // The smallest unused id after the call wins: a real result is
                    // appended right after its call, while a synthesized one is
                    // appended at the tail, so a real result is always preferred
                    // without needing to inspect its content.
                    let candidate = results.get(call.id.as_str()).and_then(|rows| {
                        rows.iter()
                            .filter(|r| r.id > row.id && !used.contains(&r.id))
                            .min_by_key(|r| r.id)
                            .copied()
                    });
                    match candidate {
                        // A result that predates the call cannot answer it; treat
                        // it as missing rather than attaching it backwards.
                        Some(result) => {
                            used.insert(result.id);
                            messages.push(result.to_message());
                        }
                        None => {
                            messages.push(Message::tool_result(
                                call.id.clone(),
                                call.name.clone(),
                                MISSING_TOOL_RESULT,
                            ));
                            synthesized.push(Synthesized {
                                run_id: row.run_id.clone(),
                                call,
                            });
                        }
                    }
                }
            }
            // Tool rows are emitted only through the assistant above: one whose
            // call id is not in the window (or that has already been emitted) is
            // an orphan or a duplicate.
            "tool" => {}
            _ => messages.push(row.to_message()),
        }
    }

    (messages, synthesized)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, role: &str, content: &str) -> StoredMessage {
        StoredMessage {
            id,
            session_id: "s".into(),
            run_id: Some("r".into()),
            agent_id: None,
            role: role.into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
            created_at: id,
        }
    }

    fn assistant_call(id: i64, call_id: &str) -> StoredMessage {
        let mut row = row(id, "assistant", "");
        row.tool_calls = vec![ToolCall {
            id: call_id.into(),
            name: "echo".into(),
            arguments: "{}".into(),
        }];
        row
    }

    fn tool_result(id: i64, call_id: &str, content: &str) -> StoredMessage {
        let mut row = row(id, "tool", content);
        row.tool_call_id = Some(call_id.into());
        row.name = Some("echo".into());
        row
    }

    fn roles(messages: &[Message]) -> Vec<Role> {
        messages.iter().map(|m| m.role).collect()
    }

    #[test]
    fn drops_a_tool_result_without_a_preceding_call() {
        let rows = vec![
            tool_result(1, "call_old", "result from outside the window"),
            row(2, "user", "hello"),
        ];
        let (messages, synthesized) = repair_rows(&rows);
        assert_eq!(roles(&messages), vec![Role::User]);
        assert!(synthesized.is_empty());
    }

    #[test]
    fn synthesizes_missing_results_after_their_assistant() {
        let rows = vec![
            row(1, "user", "go"),
            assistant_call(2, "call_1"),
            row(3, "user", "retry"),
        ];
        let (messages, synthesized) = repair_rows(&rows);
        assert_eq!(
            roles(&messages),
            vec![Role::User, Role::Assistant, Role::Tool, Role::User]
        );
        assert_eq!(messages[1].tool_calls[0].id, "call_1");
        assert_eq!(messages[2].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(messages[2].content, MISSING_TOOL_RESULT);
        assert_eq!(messages[3].content, "retry");
        assert_eq!(synthesized.len(), 1);
        assert_eq!(synthesized[0].call.id, "call_1");
    }

    #[test]
    fn reuses_a_persisted_synthesized_result_and_places_it_correctly() {
        // What the previous repair left on disk: the synthesized row was
        // appended at the tail, after the follow-up user turn.
        let rows = vec![
            row(1, "user", "go"),
            assistant_call(2, "call_1"),
            row(3, "user", "retry"),
            tool_result(4, "call_1", MISSING_TOOL_RESULT),
        ];
        let (messages, synthesized) = repair_rows(&rows);
        assert_eq!(
            roles(&messages),
            vec![Role::User, Role::Assistant, Role::Tool, Role::User]
        );
        assert_eq!(
            messages[2].tool_call_id.as_deref(),
            Some("call_1"),
            "the tail row must be relocated, not dropped"
        );
        assert!(synthesized.is_empty(), "must not append a second copy");
    }

    #[test]
    fn keeps_parallel_results_in_call_order() {
        let mut assistant = row(1, "assistant", "");
        assistant.tool_calls = vec![
            ToolCall {
                id: "a".into(),
                name: "x".into(),
                arguments: "{}".into(),
            },
            ToolCall {
                id: "b".into(),
                name: "y".into(),
                arguments: "{}".into(),
            },
        ];
        let rows = vec![
            assistant,
            tool_result(2, "b", "second"),
            tool_result(3, "a", "first"),
        ];
        let (messages, synthesized) = repair_rows(&rows);
        assert_eq!(
            roles(&messages),
            vec![Role::Assistant, Role::Tool, Role::Tool]
        );
        assert_eq!(messages[1].content, "first");
        assert_eq!(messages[2].content, "second");
        assert!(synthesized.is_empty());
    }

    #[test]
    fn keeps_the_final_user_message_and_ignores_duplicate_results() {
        let rows = vec![
            row(1, "user", "go"),
            assistant_call(2, "call_1"),
            tool_result(3, "call_1", "done"),
            tool_result(4, "call_1", "duplicate"),
            row(5, "user", "last"),
        ];
        let (messages, synthesized) = repair_rows(&rows);
        assert_eq!(
            roles(&messages),
            vec![Role::User, Role::Assistant, Role::Tool, Role::User]
        );
        assert_eq!(messages[2].content, "done");
        assert_eq!(messages[3].content, "last");
        assert!(synthesized.is_empty());
    }
}
