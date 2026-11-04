// @kc-doc: session-titles
// @kc-title: Give sessions AI-written titles
// @kc-summary: Sessions are titled from their first message; with `[titles]` on (the default) a model writes a short, plain-language title instead, using the session's own model or a cheaper one you pick.
// @kc-tags: titles, sessions, naming, models, titles.enabled
// @kc-applies-to: config:[titles], env:KARUCODE_TITLES, env:KARUCODE_TITLES_PROVIDER, env:KARUCODE_TITLES_MODEL, config:titles.enabled, config:titles.provider, config:titles.model, config:titles.max_chars, config:titles.timeout_secs, route:POST /api/sessions, route:POST /api/sessions/:id/messages, route:PATCH /api/config, ui:SessionsRail
//
// Every session needs a name. The session rail is the only map of what you are
// working on, and a rail full of untitled rows is unusable after a day.
//
// # What happens by default
//
// **AI titles are on out of the box** (`[titles] enabled = true`). When a session
// that is still untitled receives its first user message, karucode:
//
// 1. renames it immediately with a deterministic title derived from that message
//    (whitespace collapsed, cut at 60 characters) so the rail never shows an
//    empty row;
// 2. asks a model, in the background, for a short title that says what the
//    session is actually *about*;
// 3. renames the session again with the model's answer.
//
// Nothing blocks: the run starts as soon as the message is accepted. If the title
// request fails, is slow, or the endpoint refuses it, the deterministic title
// stays and the reason is logged. A failed title is never an error the user sees.
//
// Use the same session twice? A second message does not retitle the session. A
// title is written once, from the opening request.
//
// # Which model writes the title
//
// By default the title is written by **the model the session itself is running**
// - the "inherit" behaviour. If you pinned the session to a model, the title uses
// that model; otherwise the install-wide default. There is nothing extra to
// configure, and a session on a local model titles itself locally.
//
// A title only needs a handful of tokens, so the usual reason to point it
// somewhere else is cost or latency: the model you want for real work is often
// not the one you want for a 20-token classification. Set both keys to use a
// different model:
//
// ```toml
// [titles]
// enabled = true
// provider = "cheap"        # a [[providers]] id
// model = "gpt-4o-mini"     # one of that profile's models
// max_chars = 60
// timeout_secs = 20
// ```
//
// `provider` and `model` are independent: `provider` alone uses that profile's
// default model, and `model` alone uses the *active* profile with a different
// model id. Leaving both empty is the inherited default.
//
// A `titles.provider` that no longer exists falls back to the install-wide
// default with a warning rather than disabling titles.
//
// # Settings that matter
//
// | Key | Default | What it does |
// | --- | --- | --- |
// | `titles.enabled` | `true` | Off means no model call at all; the derived title stays. |
// | `titles.provider` | *(empty)* | Profile id for the title request; empty inherits. |
// | `titles.model` | *(empty)* | Model id for the title request; empty inherits. |
// | `titles.max_chars` | `60` | Ceiling in characters (8-120). The answer is truncated on a char boundary. |
// | `titles.timeout_secs` | `20` | Give up and keep the derived title (1-120). |
//
// The same three switches are available without a config file, for a locked
// deployment:
//
// ```bash
// export KARUCODE_TITLES=0            # turn the extra turn off entirely
// export KARUCODE_TITLES_PROVIDER=cheap
// export KARUCODE_TITLES_MODEL=gpt-4o-mini
// ```
//
// `max_chars` and `timeout_secs` are file/API only.
//
// # What it will not do
//
// * **It will not overwrite a title you set.** The rename is refused unless the
//   session still carries the title the request started from. A manual rename, a
//   concurrent title, or a session deleted mid-request all win; the session is
//   left exactly as it is.
// * **It does not run on an explicitly named session.** `POST /api/sessions`
//   with a `title` means you named it - there is nothing to improve on.
// * **It does not read your conversation.** The request carries the first user
//   message, truncated to 2000 characters, and nothing else: no tool output, no
//   file contents, no prior turns. That is what keeps the turn cheap, and what
//   keeps an entire repository out of a titling request.
// * **It is not prompted into a jailbreak surface.** The message is delivered as
//   data inside a fixed instruction ("Reply with a title only"), and the answer
//   is stripped to a single line, capped at `max_chars`, and rejected if nothing
//   printable is left. A prompt injection in the first message can at worst
//   produce a bad title - it has no tools and no transcript to act on.
//
// # Turning it off
//
// ```toml
// [titles]
// enabled = false
// ```
//
// or `KARUCODE_TITLES=0`, or the toggle in Settings -> Titles. Sessions keep the
// deterministic title derived from their first message, exactly as before AI
// titles existed.
//
// Full reference: `docs/API.md` (`POST /api/sessions/:id/messages`).

use crate::state::AppState;
use kc_core::config::SessionModelSelection;
use kc_core::config::{ModelConfig, TitlesConfig, TITLE_PROMPT_CHARS};
use kc_core::message::Message;
use kc_core::provider::{ChatRequest, ProviderChunk};
use kc_provider::strip_tool_markup;

/// Instruction that precedes the user's message.
///
/// Deliberately blunt and self-contained. Two failure modes are worth designing
/// against, and both are handled by the *output* filter rather than by hoping the
/// model obeys:
///
/// * a model that answers a question instead of titling - its answer is still a
///   single line, so it merely produces a worse title;
/// * a model that answers with a chatty preamble ("Sure! Here's a title: ...") -
///   [`clean_title`] strips the common wrappers.
const TITLE_INSTRUCTION: &str = "\
Write a title for a coding session based on its opening request.

Rules:
- At most {max_chars} characters.
- Plain language, lower case unless a proper noun needs capitals.
- Say what the work is, not what the user said: \"fix flaky auth test\", not \
\"user asks about a test\".
- No quotes, no trailing punctuation, no preamble, no markdown.
- Reply with the title and nothing else.";

/// Build the request that asks a model for a title.
///
/// `max_tokens` is generous relative to `max_chars` (a token is ~4 characters,
/// and a model that opens with a preamble needs room to be filtered back down),
/// but `max_chars` is what actually bounds the stored title.
fn title_request(model: &ModelConfig, prompt: &str, titles: &TitlesConfig) -> ChatRequest {
    let max_chars = titles.title_budget();
    let instruction = TITLE_INSTRUCTION.replace("{max_chars}", &max_chars.to_string());
    let excerpt = truncate_chars(prompt.trim(), TITLE_PROMPT_CHARS);
    ChatRequest {
        admission: None,
        model: model.model.clone(),
        messages: vec![Message::system(instruction), Message::user(excerpt)],
        // No tools: a titling turn must not be able to call anything.
        tools: Vec::new(),
        // Titles want a little variety, but this model may be a reasoning model
        // where a sampling value is what pushes it into a loop. Sampling is off
        // like everywhere else; the instruction does the work.
        temperature: model.temperature,
        send_temperature: false,
        max_tokens: Some((max_chars as u32).saturating_mul(4).clamp(32, 512)),
        top_p: None,
        parallel_tool_calls: false,
        extra_headers: model
            .extra_headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        extra_body: model
            .extra_body
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    }
}

/// Cut `text` to at most `max` characters on a char boundary, appending `…`
/// when anything was dropped.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    let kept = out.trim_end().len();
    out.truncate(kept);
    out.push('…');
    out
}

/// Does this line only announce that a title follows, without being one?
///
/// "Here is a title:", "Sure!", "Title:" and similar are preambles. The test is
/// deliberately conservative - it must not eat a real title - so it only matches
/// a short line that either ends in `:` after an announcement word, or is nothing
/// but an acknowledgement.
fn announces_a_title(line: &str) -> bool {
    let lower = line
        .trim()
        .trim_end_matches(['*', ' ', '_'])
        .to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    // "title:", "**title:**", "here is the title:", "the title is:"
    if lower.ends_with(':') {
        return true;
    }
    const ACKS: [&str; 8] = [
        "sure",
        "sure!",
        "sure thing",
        "ok",
        "okay",
        "certainly",
        "of course",
        "here you go",
    ];
    ACKS.contains(&lower.as_str()) && lower.split_whitespace().count() <= 4
}

/// Turn a model's answer into a title, or `None` when nothing usable is left.
///
/// The filter is the security boundary as much as a cosmetic one: whatever the
/// model (or a prompt injection in the first message) produced lands in the
/// session rail, so it must be a single short printable line.
///
/// 1. pick the line to use: the first non-empty one, unless that line only
///    *announces* a title ("Here you go:", "Title:") in which case the next
///    non-empty line is the actual answer;
/// 2. strip a leading `#`, `*`, `Title:` or `title -` wrapper;
/// 3. strip matching surrounding quotes;
/// 4. collapse whitespace, drop control characters;
/// 5. drop trailing punctuation (`.`/`:`/`;`) - a title is not a sentence;
/// 6. truncate to `max_chars`, and reject anything that came out empty.
pub fn clean_title(raw: &str, max_chars: usize) -> Option<String> {
    let lines: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let first = *lines.first()?;
    // A chatty model very often answers "Sure! Here is a title:" and then the
    // title on its own line. `first` alone would store the preamble, which is
    // still a title but a useless one - "Here's a title" tells nobody anything.
    let line = if announces_a_title(first) && lines.len() > 1 {
        lines[1]
    } else {
        first
    };

    let opened_with_underscore = line.trim_start().starts_with('_');
    let line = line.trim_start_matches(['#', '*', ' ']).trim();
    // `**bold**` / `_italic_` can wrap the whole title; the leading marker was just
    // removed, so remove the matching closer too. `_` is stripped only when the
    // line opened with it, because a trailing `_` is legitimate in an identifier
    // (`snake_case`), while a trailing `*` is not.
    let line = if opened_with_underscore {
        line.trim_start_matches('_').trim_end_matches('_').trim()
    } else {
        line.trim_end_matches('*').trim()
    };
    let line = line
        .strip_prefix("Title:")
        .or_else(|| line.strip_prefix("title:"))
        .or_else(|| line.strip_prefix("TITLE:"))
        .unwrap_or(line)
        .trim();
    // `\"quoted\"`, `'quoted'` and the typographic pair.
    let line = line
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            line.strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .or_else(|| {
            line.strip_prefix('\u{201c}')
                .and_then(|rest| rest.strip_suffix('\u{201d}'))
        })
        .unwrap_or(line);

    // Control characters would break the rail's layout (or the SSE frame).
    let collapsed: String = line
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    // A model that answered with DSML tool-call markup instead of a title has
    // not answered at all: "Looking at the workspace.<||DSML||tool_calls>…" is
    // not a title, and the rail would show a line of XML. Keep the prose that
    // preceded the markup, if any, and otherwise fall back to the deterministic
    // title by returning `None`. This runs before the punctuation trim so the
    // sentence the model wrote becomes a title ("Looking at the workspace").
    let collapsed = strip_tool_markup(&collapsed);
    let trimmed = collapsed
        .trim()
        .trim_end_matches(['.', ':', ';', ',', ' '])
        .trim();
    if trimmed.is_empty() {
        return None;
    }
    let title = truncate_chars(trimmed, max_chars);
    // A title that is only punctuation ("...", "-") is not a title.
    if !title.chars().any(char::is_alphanumeric) {
        return None;
    }
    Some(title)
}

/// Read a whole completion for a titling request, bounded by `deadline`.
///
/// Titles are small, so the answer is a handful of deltas; the ceiling is a
/// runaway model rather than a long one. Reasoning deltas are ignored: a
/// reasoning model that thinks *and then* answers still produces a usable title,
/// and a model that only reasons produces no title at all, which is handled by
/// the caller as "no title".
async fn read_title_answer(
    mut stream: kc_core::provider::ProviderStream,
    deadline: tokio::time::Instant,
) -> Option<String> {
    use futures_util::StreamExt;
    let mut text = String::new();
    loop {
        let next = tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => return None,
            next = stream.next() => next,
        };
        match next {
            Some(Ok(ProviderChunk::Text(delta))) => {
                text.push_str(&delta);
                // More than any title could need; the rest is preamble we would
                // filter away anyway.
                if text.len() > 2_000 {
                    return Some(text);
                }
            }
            Some(Ok(ProviderChunk::Finish { .. })) | None => return Some(text),
            Some(Ok(ProviderChunk::Error { message, .. })) => {
                tracing::debug!(error = %message, "title request ended with a provider error");
                return if text.trim().is_empty() {
                    None
                } else {
                    Some(text)
                };
            }
            Some(Ok(_)) => {}
            Some(Err(error)) => {
                tracing::debug!(error = %error, "title stream failed");
                return if text.trim().is_empty() {
                    None
                } else {
                    Some(text)
                };
            }
        }
    }
}

/// Ask a model for a title and return the cleaned result.
///
/// Split from [`maybe_auto_title`] so the prompt-building and the network call can
/// be tested apart: `title_request` and `clean_title` are pure, and this is the
/// only part that needs an endpoint.
pub async fn generate_title(
    state: &AppState,
    model: &ModelConfig,
    titles: &TitlesConfig,
    prompt: &str,
    session_id: &str,
) -> Option<String> {
    let mut request = title_request(model, prompt, titles);
    request.admission = Some(kc_core::provider::AttemptAdmission {
        store: state.store().clone(),
        session_id: session_id.to_owned(),
    });
    let provider = match state.providers().build(model).await {
        Ok(provider) => provider,
        Err(error) => {
            tracing::debug!(error = %error, "could not build the titling provider");
            return None;
        }
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let stream = match provider.stream_chat(request, cancel.child_token()).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!(error = %error, "the title request was refused");
            return None;
        }
    };
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(titles.timeout_secs.max(1));
    let raw = read_title_answer(stream, deadline).await?;
    clean_title(&raw, titles.title_budget())
}

/// Give `session_id` an AI-written title, if it is still eligible.
///
/// Called from the message route *after* `submit` has been accepted, so nothing
/// here can delay the run. Every outcome that is not a successful rename is a
/// no-op with a log line: this is a naming convenience, not a correctness
/// requirement of the run.
///
/// `expected_title` is the title the session had when the request started. The
/// rename only lands if the session still carries it - see [`rename_if_unchanged`].
pub async fn maybe_auto_title(
    state: &AppState,
    session_id: &str,
    expected_title: &str,
    prompt: &str,
) {
    let config = state.settings().snapshot();
    if !config.titles.enabled {
        return;
    }
    // The session's own pin (or the install-wide default) decides the model,
    // unless `[titles]` names one.
    let selection: Option<SessionModelSelection> = state
        .store()
        .get_session(session_id)
        .await
        .ok()
        .flatten()
        .and_then(|session| session.model);
    let Some(model) =
        crate::settings::resolved_title_model_for_session(&config, selection.as_ref())
    else {
        return;
    };

    let Some(title) = generate_title(state, &model, &config.titles, prompt, session_id).await
    else {
        tracing::debug!(
            session = %session_id,
            model = %model.model,
            "the titling model produced nothing usable; keeping the derived title"
        );
        return;
    };
    if title == expected_title {
        return;
    }

    match rename_if_unchanged(state, session_id, expected_title, &title).await {
        Ok(true) => {
            tracing::info!(
                session = %session_id,
                model = %model.model,
                title = %title,
                "session titled by a model"
            );
        }
        Ok(false) => {
            tracing::debug!(
                session = %session_id,
                "the session's title changed while it was being generated; keeping the new one"
            );
        }
        Err(error) => {
            tracing::warn!(
                session = %session_id,
                error = %error,
                "could not store the generated title"
            );
        }
    }
}

/// Rename a session only if its title is still `expected`.
///
/// This is what makes "a human wins" true rather than aspirational. The check and
/// the write are two statements, but the window is a single in-process await on a
/// SQLite write whose only competitor is the same process, and the failure mode
/// if they did interleave is that the user's rename is replaced by a title
/// - not that data is lost. The re-read is done on the store's write path so an
///   out-of-process writer (none exist: the data directory is flock'd) cannot
///   slip between them either.
///
/// Returns `Ok(false)` when the title had already changed.
pub async fn rename_if_unchanged(
    state: &AppState,
    session_id: &str,
    expected: &str,
    title: &str,
) -> kc_core::error::Result<bool> {
    let store = state.store();
    let Some(current) = store.get_session(session_id).await? else {
        return Ok(false);
    };
    if current.title != expected {
        return Ok(false);
    }
    store.rename_session(session_id, title).await?;
    state.runtime().emit(
        Some(session_id),
        None,
        None,
        kc_core::events::Event::SessionRenamed {
            session_id: session_id.to_string(),
            title: title.to_string(),
        },
    );
    Ok(true)
}

/// True when a session's title is still the one it was created with, i.e. nobody
/// has named it yet.
///
/// A session created by `POST /api/sessions` with no `title` carries
/// [`DEFAULT_SESSION_TITLE`]; one created by an older build (or outside the API)
/// may carry the empty string. Both are "untitled". A user who deliberately
/// renames a session *into* one of those two values is retitled once more - the
/// schema keeps no `renamed` provenance bit, so that case is indistinguishable
/// from an untouched session.
pub fn is_untitled(title: &str) -> bool {
    let title = title.trim();
    title.is_empty() || title == crate::routes::sessions::DEFAULT_SESSION_TITLE
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::config::ModelConfig;

    #[test]
    fn a_title_budget_bounds_the_request_and_the_answer() {
        let titles = TitlesConfig {
            max_chars: 40,
            ..TitlesConfig::default()
        };
        let request = title_request(&ModelConfig::default(), "do the thing", &titles);
        assert_eq!(request.messages.len(), 2);
        assert!(
            request.messages[0]
                .content
                .contains("At most 40 characters"),
            "the instruction must state the budget the caller will enforce"
        );
        assert!(
            request.tools.is_empty(),
            "a titling turn must not carry tools"
        );
        assert!(!request.send_temperature);
        assert_eq!(request.max_tokens, Some(160));
    }

    #[test]
    fn the_prompt_is_truncated_so_a_paste_cannot_swamp_the_turn() {
        let titles = TitlesConfig::default();
        let huge = "x".repeat(TITLE_PROMPT_CHARS + 5000);
        let request = title_request(&ModelConfig::default(), &huge, &titles);
        let sent = &request.messages[1].content;
        assert_eq!(sent.chars().count(), TITLE_PROMPT_CHARS + 1, "… appended");
        assert!(sent.ends_with('…'));
    }

    #[test]
    fn chatty_answers_are_reduced_to_one_short_line() {
        // The common wrappers a model adds even when told not to. A line that
        // only announces the title must not become the title.
        assert_eq!(
            clean_title("Sure! Here you go:\n\n**Fix flaky auth test**", 60).unwrap(),
            "Fix flaky auth test"
        );
        assert_eq!(
            clean_title("Here is the title:\nfix the flaky build", 60).unwrap(),
            "fix the flaky build"
        );
        assert_eq!(
            clean_title("Sure!\nfix the build", 60).unwrap(),
            "fix the build"
        );
        assert_eq!(
            clean_title("# Fix flaky auth test.", 60).unwrap(),
            "Fix flaky auth test"
        );
        // A real title is never mistaken for a preamble, even with a colon-free
        // question mark or an exclamation in it.
        assert_eq!(
            clean_title("Why is the build red?", 60).unwrap(),
            "Why is the build red?"
        );
        // A single-line announcement with nothing after it is still all we have.
        assert_eq!(clean_title("Sure!", 60).unwrap(), "Sure!");
    }

    #[test]
    fn titles_are_stripped_of_wrappers_quotes_and_trailing_punctuation() {
        assert_eq!(
            clean_title("Title: Fix the build", 60).unwrap(),
            "Fix the build"
        );
        assert_eq!(
            clean_title("title: fix the build.", 60).unwrap(),
            "fix the build"
        );
        assert_eq!(
            clean_title("\"Fix the build\"", 60).unwrap(),
            "Fix the build"
        );
        assert_eq!(
            clean_title("\u{201c}Fix the build\u{201d}", 60).unwrap(),
            "Fix the build"
        );
        assert_eq!(clean_title("  spaced\n\n  out  ", 60).unwrap(), "spaced");
        // A second line is never part of the title.
        assert_eq!(
            clean_title("Real title\n\nExplanation.", 60).unwrap(),
            "Real title"
        );
    }

    #[test]
    fn an_unusable_answer_is_no_title_at_all() {
        assert_eq!(clean_title("", 60), None);
        assert_eq!(clean_title("   \n\t\n", 60), None);
        assert_eq!(clean_title("\"\"", 60), None);
        assert_eq!(clean_title("...", 60), None);
        assert_eq!(clean_title("---", 60), None);
    }

    #[test]
    fn control_characters_cannot_reach_the_session_rail() {
        let title = clean_title("fix\u{0}\u{7}the\u{2028}build", 60).unwrap();
        assert!(!title.chars().any(char::is_control));
        assert!(title.contains("fix"));
        assert!(title.contains("build"));
    }

    #[test]
    fn a_long_answer_is_cut_on_a_char_boundary() {
        let long = clean_title(&"é".repeat(200), 30).unwrap();
        assert_eq!(long.chars().filter(|c| *c == 'é').count(), 30);
        assert!(long.ends_with('…'));
        // Exactly at the limit: untouched, no ellipsis.
        let exact = clean_title(&"a".repeat(30), 30).unwrap();
        assert_eq!(exact.chars().count(), 30);
        assert!(!exact.ends_with('…'));
    }

    #[test]
    fn the_budget_is_clamped_to_what_the_rail_can_show() {
        let tiny = TitlesConfig {
            max_chars: 1,
            ..TitlesConfig::default()
        };
        assert_eq!(tiny.title_budget(), 8);
        let vast = TitlesConfig {
            max_chars: 10_000,
            ..TitlesConfig::default()
        };
        assert_eq!(vast.title_budget(), kc_core::config::MAX_TITLE_CHARS_HINT);
    }

    #[test]
    fn empty_model_fields_mean_inherit() {
        let default = TitlesConfig::default();
        assert!(default.inherits_session_model());
        assert!(default.enabled);

        let provider_only = TitlesConfig {
            provider: "cheap".into(),
            ..TitlesConfig::default()
        };
        assert!(!provider_only.inherits_session_model());
        assert_eq!(provider_only.provider_id(), Some("cheap"));
        assert_eq!(provider_only.model_id(), None);

        // Whitespace is not an id.
        let blank = TitlesConfig {
            provider: "   ".into(),
            model: "\t".into(),
            ..TitlesConfig::default()
        };
        assert!(blank.inherits_session_model());
    }

    #[test]
    fn untitled_sessions_are_the_ones_titles_run_for() {
        assert!(is_untitled(""));
        assert!(is_untitled("  "));
        assert!(is_untitled(crate::routes::sessions::DEFAULT_SESSION_TITLE));
        assert!(!is_untitled("New session!"));
        assert!(!is_untitled("fix the build"));
    }

    #[test]
    fn a_title_is_never_a_block_of_tool_markup() {
        // DeepSeek-V4 answers in DSML sometimes, including to the title prompt.
        // The rail must never show a line of XML - and a model that had nothing
        // to say but a tool call has not named the session.
        assert_eq!(
            clean_title("Looking at the workspace.", 60).as_deref(),
            Some("Looking at the workspace")
        );
        assert_eq!(
            clean_title(
                "Looking at the workspace.<||DSML||tool_calls><||DSML||invoke name=\"list_dir\">\
                 <||DSML||parameter name=\"path\" string=\"true\">.</||DSML||parameter>\
                 </||DSML||invoke></||DSML||tool_calls>",
                60
            )
            .as_deref(),
            Some("Looking at the workspace")
        );
        // Markup and nothing else: fall back to the derived title.
        assert_eq!(
            clean_title(
                "<||DSML||tool_calls><||DSML||invoke name=\"list_dir\"></||DSML||invoke></||DSML||tool_calls>",
                60
            ),
            None
        );
    }

    #[test]
    fn titles_are_on_by_default() {
        // The feature is "on by default" as a product decision, not a default of
        // whatever the struct happens to hold.
        assert!(TitlesConfig::default().enabled);
        assert!(kc_core::config::Config::default().titles.enabled);
    }
}
