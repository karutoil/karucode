//! Slash commands: the `/` verbs a session understands.
//
// @kc-doc: slash-commands
// @kc-title: Type a slash command in a session
// @kc-summary: A message that starts with `/` runs a command - compact the conversation, pin a model, run a skill, list the MCP tools - instead of prompting the model.
// @kc-tags: slash, commands, composer, compact, skills, mcp, tools, web-ui
// @kc-applies-to: route:GET /api/commands, route:POST /api/sessions/:id/messages, field:content, event:command_result, tool:skill, tool:karucode_docs, config:[skills], config:[[mcp_servers]]
//
// # Where commands are typed
//
// In the message box, as the whole message. The web composer opens a menu the
// moment the draft starts with `/` (fed by `GET /api/commands`), `↑`/`↓` and
// `Enter` pick a row, and the message is sent exactly like any other - the
// server decides whether it is a command. Nothing is client-side only: an API
// client, a control chat or a script can post the same text to
// `POST /api/sessions/:id/messages` and get the same result.
//
// A command must be the **first** token of the message, which is why
// `/home/me/notes.md` and a pasted path are ordinary prompts. Name matching is
// case-insensitive.
//
// # What comes back
//
// Most commands answer in place: the result is written to the transcript as a
// message of its own (tagged `slash:<command>` in the `name` column, which is
// what the chat draws as a command card) and published as a `command_result`
// event. It is **not** sent to the model later - it is an answer for the reader,
// not a turn in the conversation - so `/help` or `/skills` never costs a token.
//
// The response is `200` with `{"mode": "command", "command": "...",
// "message_id": N, "state": "ok"}`. `state` is `"error"` when the command
// refused, and the card says why - a refusal is a result, not an HTTP error,
// because it is something the reader needs to see next to what they typed.
//
// ```bash
// curl -s -X POST localhost:8787/api/sessions/s_abc/messages \
//   -H 'X-Karucode-Client: web' -H 'Content-Type: application/json' \
//   -d '{"content": "/compact"}'
// ```
//
// # The commands
//
// | Command | What it does |
// | --- | --- |
// | `/help [command]` | every command, or one in detail |
// | `/compact` | compact the conversation now instead of waiting for the budget |
// | `/context` | the window, the budget and what the session is spending |
// | `/model [provider/model]` | show, pin or clear this session's model |
// | `/effort [level]` | show or set the reasoning effort (`none`, `minimal`, `low`, `medium`, `high`, `max`) |
// | `/rename <title>` | rename this session |
// | `/stop` | stop the run in flight |
// | `/skills` | the skill library |
// | `/skill <name> [task]` | run a skill (see below) |
// | `/mcp` | MCP servers and the tools they contribute |
// | `/tools` | every tool the agent can call |
// | `/agents` | this session's subagents |
// | `/docs <query>` | search karucode's own documentation |
//
// # Skills are commands too
//
// Every skill whose id is a valid command name is available under that name, so
// `/release` runs the `release` skill. A skill never shadows a built-in: if the
// ids collide the skill is still reachable as `/skill <id>`, and the menu shows
// it that way.
//
// A skill invocation does not answer in place - it **rewrites the message** into
// a normal prompt that names the skill and carries the task you typed, and that
// prompt runs. So `/skill review the diff in src/api` sends "read the `review`
// skill ... the diff in src/api" as your turn, which is exactly what you would
// have typed by hand.
//
// # Limits worth knowing
//
// * A command is answered in **any** message mode: `?mode=steer` and
//   `?mode=follow_up` still run it rather than queueing it, so `/stop` works
//   while a run is working.
// * `/compact` is refused while a run is in flight - that run holds its own copy
//   of the conversation and would write it back over the compaction. `/model` and
//   `/effort` are accepted at any time: they change what the *next* run uses.
// * A single-token message that matches no command (`/compcat`) is a `400`
//   naming the closest commands. Anything with more text after the token is sent
//   as an ordinary prompt, because that is what a path looks like.
// * Every command is bounded: `/docs` returns five matches, `/tools` and `/agents`
//   cap their listings, and a command's arguments are refused rather than
//   truncated above 4,000 characters.

use crate::error::ApiResult;
use crate::state::AppState;
use kc_core::events::Event;
use kc_core::paths::Workspace;
use kc_core::store::{NewMessage, Session, COMMAND_ERROR_SUFFIX, COMMAND_MESSAGE_PREFIX};
use kc_core::Settings;
use serde::Serialize;

mod commands;
pub use commands::discover_skills;

/// Longest argument string a command will look at, in characters.
///
/// A command's arguments are a title, a model id, a skill name or a search
/// query. Anything longer is a paste that was meant for the model, and refusing
/// it with a number is better than silently cutting a message in half.
pub const MAX_COMMAND_ARGS_CHARS: usize = 4_000;

/// A parsed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The command name without its slash, lowercased.
    pub name: String,
    /// Everything after the name, trimmed. Empty when the command takes none.
    pub args: String,
}

impl Invocation {
    pub fn args_trimmed(&self) -> &str {
        self.args.trim()
    }

    pub fn has_args(&self) -> bool {
        !self.args_trimmed().is_empty()
    }

    /// The argument for a command that takes exactly one, or the reason it is
    /// missing.
    pub fn required_arg(&self, usage: &str) -> std::result::Result<&str, String> {
        let arg = self.args_trimmed();
        if arg.is_empty() {
            return Err(format!("this command needs an argument: `{usage}`"));
        }
        Ok(arg)
    }
}

/// Parse a message as a slash command.
///
/// Returns `None` for anything that is not a command, and that includes things
/// that merely *start* with a slash: a pasted `/home/me/file.ts`, a `// comment`,
/// and a bare `/` are all ordinary messages. The rules are narrow on purpose,
/// because a false positive turns a prompt into a refusal:
///
/// * the `/` must be the very first byte - no leading whitespace, so an indented
///   code block is never a command;
/// * the name is ASCII alphanumerics, `-` and `_`, and must start with a letter;
/// * what follows must be the end of the message or whitespace. `/compact!` is a
///   prompt, not a malformed `/compact`.
pub fn parse(content: &str) -> Option<Invocation> {
    let rest = content.strip_prefix('/')?;
    let name_len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or(rest.len());
    let name = &rest[..name_len];
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let tail = &rest[name_len..];
    if !(tail.is_empty() || tail.starts_with(char::is_whitespace)) {
        return None;
    }
    Some(Invocation {
        name: name.to_ascii_lowercase(),
        args: tail.trim().to_string(),
    })
}

/// Where a command comes from, so the menu can say why it is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Shipped with the server.
    Builtin,
    /// A skill found in `.karucode/skills`, `.claude/skills`, a configured
    /// folder or the settings table.
    Skill,
    /// A `command` contribution from a ready plugin (§R3). It runs that plugin's
    /// `ui/action` (or posts its `message`), costs no tokens and needs no
    /// approval: it is typed by the person, not proposed by the model.
    Plugin,
}

/// The menu group a command belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Session,
    Context,
    Extend,
    Inspect,
    Meta,
}

impl Category {
    pub fn title(self) -> &'static str {
        match self {
            Category::Session => "Session",
            Category::Context => "Context",
            Category::Extend => "Extend",
            Category::Inspect => "Inspect",
            Category::Meta => "Help",
        }
    }
}

/// One row of `GET /api/commands`, and one row of the composer's menu.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// The name without its slash.
    pub name: String,
    pub title: String,
    pub summary: String,
    /// The argument placeholder (`"<title>"`), or empty when it takes none.
    pub args: String,
    pub category: Category,
    pub source: Source,
    /// Exactly what the composer inserts when the row is picked, including the
    /// trailing space a command with arguments needs.
    pub insert: String,
}

/// A command the server ships with.
///
/// `handler` is not a function pointer: the match in [`run`] is exhaustive over
/// this table, so a name added here without a branch is a compile error rather
/// than a menu row that does nothing.
struct Builtin {
    name: &'static str,
    title: &'static str,
    summary: &'static str,
    args: &'static str,
    category: Category,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "help",
        title: "List every command",
        summary: "What each slash command does, and how to use one",
        args: "[command]",
        category: Category::Meta,
    },
    Builtin {
        name: "compact",
        title: "Compact the conversation",
        summary: "Fold the older turns into one digest now, instead of waiting for the budget",
        args: "",
        category: Category::Context,
    },
    Builtin {
        name: "context",
        title: "Show the context budget",
        summary: "The model, its window, the character budget and what this session is using",
        args: "",
        category: Category::Context,
    },
    Builtin {
        name: "model",
        title: "Show or pin the model",
        summary: "Pin this session to one model, or clear the pin to follow the default",
        args: "[provider/model]",
        category: Category::Session,
    },
    Builtin {
        name: "effort",
        title: "Show or set reasoning effort",
        summary: "Pin the reasoning effort for this session's runs",
        args: "[level]",
        category: Category::Session,
    },
    Builtin {
        name: "rename",
        title: "Rename this session",
        summary: "Give the current session a title",
        args: "<title>",
        category: Category::Session,
    },
    Builtin {
        name: "stop",
        title: "Stop the run in flight",
        summary: "Cancel the run that is working right now",
        args: "",
        category: Category::Session,
    },
    Builtin {
        name: "skills",
        title: "List the skill library",
        summary: "Every skill the agent can load, with where it came from",
        args: "",
        category: Category::Extend,
    },
    Builtin {
        name: "skill",
        title: "Run a skill",
        summary: "Send a message that tells the agent to read one skill and follow it",
        args: "<name> [task]",
        category: Category::Extend,
    },
    Builtin {
        name: "mcp",
        title: "List MCP servers",
        summary: "The MCP servers this install runs and the tools each contributes",
        args: "",
        category: Category::Extend,
    },
    Builtin {
        name: "tools",
        title: "List the tools",
        summary: "Every tool the agent can call, grouped by where it comes from",
        args: "",
        category: Category::Extend,
    },
    Builtin {
        name: "agents",
        title: "List this session's subagents",
        summary: "Subagents spawned in this session, with status and iterations",
        args: "",
        category: Category::Inspect,
    },
    Builtin {
        name: "docs",
        title: "Search karucode's documentation",
        summary: "Find the built-in page about a config key, a route, a tool or a plugin",
        args: "<query>",
        category: Category::Inspect,
    },
];

impl Builtin {
    fn entry(&self) -> Entry {
        Entry {
            name: self.name.to_string(),
            title: self.title.to_string(),
            summary: self.summary.to_string(),
            args: self.args.to_string(),
            category: self.category,
            source: Source::Builtin,
            insert: if self.args.is_empty() {
                format!("/{}", self.name)
            } else {
                format!("/{} ", self.name)
            },
        }
    }
}

/// Every built-in command, in menu order.
pub fn builtins() -> impl Iterator<Item = Entry> {
    BUILTINS.iter().map(Builtin::entry)
}

/// True when `name` is a built-in command.
pub fn is_builtin(name: &str) -> bool {
    BUILTINS.iter().any(|builtin| builtin.name == name)
}

/// Is this string a command name a skill could be invoked by (`/id`)?
///
/// A skill id that is not usable here is still available as `/skill <id>`; the
/// two rules only have to agree that a name is lowercase ASCII, digits, `-` and
/// `_`, starting with a letter - the same shape [`parse`] accepts.
pub fn is_command_token(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The command catalog: built-ins first, then every skill that can be typed.
///
/// Skills are discovered from disk on every call, exactly like `GET /api/skills`:
/// there is no index to invalidate, so a skill file saved a second ago is in the
/// menu. A skill whose id collides with a built-in is left out, because `/help`
/// must always mean help; the skill stays reachable as `/skill <id>`.
pub async fn catalog(state: &AppState, workspace: Option<&Workspace>) -> Vec<Entry> {
    let mut entries = BUILTINS.iter().map(Builtin::entry).collect::<Vec<_>>();
    // Plugin commands are appended last and never depend on a workspace: they are
    // manifest data from a ready process, not files, so a deployment with no
    // project still gets them (§R3).
    let plugin_commands = state.plugin_commands().await;
    if let Some(workspace) = workspace {
        let config = state.settings().skills().await;
        if let Ok(registry) = discover_skills(&config, workspace).await {
            for skill in registry.skills.iter().filter(|skill| skill.enabled) {
                if !is_command_token(&skill.id) || is_builtin(&skill.id) {
                    continue;
                }
                if plugin_commands.iter().any(|c| c.0 == skill.id) {
                    // A plugin command is namespaced by the host, so this cannot
                    // happen today; keeping the check makes the menu's "one name,
                    // one command" rule true whatever the namespacing becomes.
                    continue;
                }
                entries.push(Entry {
                    name: skill.id.clone(),
                    title: if skill.name.trim().is_empty() {
                        skill.id.clone()
                    } else {
                        skill.name.clone()
                    },
                    summary: skill.description.clone(),
                    args: "[task]".to_string(),
                    category: Category::Extend,
                    source: Source::Skill,
                    insert: format!("/{} ", skill.id),
                });
            }
        }
        // `discover_skills` failing (a skills folder on a dead mount) must not
        // take the whole menu with it, so plugin commands are added below
        // regardless.
    }
    for (name, title, summary, args) in plugin_commands {
        if is_builtin(&name) {
            continue;
        }
        entries.push(Entry {
            name: name.clone(),
            title,
            summary,
            args,
            category: Category::Extend,
            source: Source::Plugin,
            insert: if name.is_empty() {
                format!("/{name}")
            } else {
                format!("/{name} ")
            },
        });
    }
    entries
}

/// The catalog entry for `name`, if any.
pub fn find<'a>(catalog: &'a [Entry], name: &str) -> Option<&'a Entry> {
    catalog
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case(name))
}

/// Up to three command names close enough to `name` to be a typo of it.
///
/// Deliberately crude (a shared prefix, or one edit apart) and deliberately not
/// clever: this is the `400` that turns `/compcat` into "did you mean
/// `/compact`?", and a wrong guess costs one reading while no guess costs a
/// confused user.
pub fn suggestions(catalog: &[Entry], name: &str) -> Vec<String> {
    let needle = name.to_ascii_lowercase();
    let mut scored: Vec<(usize, String)> = catalog
        .iter()
        .filter_map(|entry| {
            let candidate = entry.name.to_ascii_lowercase();
            let distance = edit_distance(&needle, &candidate);
            // Two edits away catches a typo and a transposition; a shared prefix
            // of three catches a name stopped halfway. Both are cheap, and both
            // stay silent about a word that has nothing in common with any command.
            let close = distance <= 2 || common_prefix(&needle, &candidate) >= 3;
            (close && needle != candidate).then(|| (distance, entry.name.clone()))
        })
        .collect();
    scored.sort();
    scored.into_iter().take(3).map(|(_, name)| name).collect()
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, left) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, right) in b.iter().enumerate() {
            let cost = usize::from(left != right);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// What running a command produced.
#[derive(Debug, Clone, Default)]
pub struct Reply {
    /// The markdown to write into the transcript. `None` means the command did
    /// not answer in place - it rewrote the message instead.
    pub text: Option<String>,
    /// When set, the command did nothing itself: the message becomes this prompt
    /// and runs exactly like something the user typed.
    pub prompt: Option<String>,
    /// Whether the command did what it says. A refusal is still a result, and is
    /// still recorded in the transcript - the reader typed it and deserves to
    /// see why nothing happened.
    pub ok: bool,
}

impl Reply {
    /// A command that answered in place.
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            prompt: None,
            ok: true,
        }
    }

    /// A command that refused, with the reason.
    pub fn fail(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            prompt: None,
            ok: false,
        }
    }

    /// A command that rewrites the message into a prompt instead of answering.
    pub fn run(prompt: impl Into<String>) -> Self {
        Self {
            text: None,
            prompt: Some(prompt.into()),
            ok: true,
        }
    }

    /// True when this reply answers in place.
    pub fn answers(&self) -> bool {
        self.text.is_some()
    }
}

/// Run one command.
///
/// `catalog` is passed in rather than rebuilt: `/help` and `/skills` need it, the
/// caller has just built it to resolve the name, and a second discovery pass would
/// be a second filesystem walk for the same answer.
pub async fn run(
    state: &AppState,
    session: &Session,
    catalog: &[Entry],
    invocation: &Invocation,
) -> ApiResult<Reply> {
    if invocation.args.chars().count() > MAX_COMMAND_ARGS_CHARS {
        return Ok(Reply::fail(format!(
            "`/{name}` takes at most {MAX_COMMAND_ARGS_CHARS} characters of arguments, and this is {} - \
             put a long instruction in an ordinary message instead.",
            invocation.args.chars().count(),
            name = invocation.name
        )));
    }
    match invocation.name.as_str() {
        "help" => commands::help(catalog, invocation).await,
        "compact" => commands::compact(state, session, invocation).await,
        "context" => commands::context(state, session, invocation).await,
        "model" => commands::model(state, session, invocation).await,
        "effort" => commands::effort(state, session, invocation).await,
        "rename" => commands::rename(state, session, invocation).await,
        "stop" => commands::stop(state, session, invocation).await,
        "skills" => commands::skills(state, session, invocation).await,
        "skill" => commands::skill(state, session, invocation).await,
        "mcp" => commands::mcp(state, session, invocation).await,
        "tools" => commands::tools(state, session, invocation).await,
        "agents" => commands::agents(state, session, invocation).await,
        "docs" => commands::docs(invocation).await,
        // Everything else that resolved in the catalog is a skill invoked by its
        // own id: `/release` is `/skill release` with nothing before the task.
        _ => match find(catalog, &invocation.name).map(|entry| entry.source) {
            // A plugin command runs the plugin's own `ui/action` (§R3). It is a
            // person typing, exactly like a click, so it takes the same path the
            // button does - and never reaches the model.
            Some(Source::Plugin) => commands::plugin(state, session, invocation).await,
            _ => {
                commands::skill_named(state, session, &invocation.name, invocation.args_trimmed())
                    .await
            }
        },
    }
}

/// The message for a single-token slash message that matches no command.
///
/// Only a message whose *whole* content is the unknown token is refused: `/x`
/// alone is never a legitimate prompt, while `/home/me/notes.md why is this
/// failing` parses as an unknown command with arguments and is deliberately sent
/// on as an ordinary message by the caller.
pub fn unknown_command_message(catalog: &[Entry], name: &str) -> String {
    let close = suggestions(catalog, name);
    let hint = match close.as_slice() {
        [] => String::new(),
        [one] => format!(" Did you mean `/{one}`?"),
        [first, rest @ ..] => format!(
            " Did you mean {}?",
            std::iter::once(first)
                .chain(rest.iter())
                .map(|name| format!("`/{name}`"))
                .collect::<Vec<_>>()
                .join(" or ")
        ),
    };
    format!("`/{name}` is not a command.{hint} Type `/help` for the whole list.")
}

/// Write a command's answer into the transcript and publish it.
///
/// The row is what makes the answer survive a reload; the event is what makes it
/// appear without one. Both are keyed by the same `message_id`, so a client that
/// sees the frame and then a transcript page can tell they are the same message
/// instead of drawing it twice.
///
/// The row is `role = assistant` with `name = "slash:<command>"`. The role is what
/// lets every existing reader draw it (the browser already renders assistant
/// turns, the transcript pager already returns them); the name is what makes it
/// recognisable as a command result to the UI *and* to the runtime, which skips
/// these rows when it builds a request. Nothing else in the schema has to change.
///
/// A refused command stores `slash:<command>:error`, so a transcript reloaded a
/// week later still knows the answer was a refusal - the state of a command is
/// not something the text alone can be trusted to imply.
pub async fn record(
    state: &AppState,
    session_id: &str,
    command: &str,
    reply: &Reply,
) -> ApiResult<i64> {
    let content = reply.text.clone().unwrap_or_default();
    let name = if reply.ok {
        format!("{COMMAND_MESSAGE_PREFIX}{command}")
    } else {
        format!("{COMMAND_MESSAGE_PREFIX}{command}{COMMAND_ERROR_SUFFIX}")
    };
    let message_id = state
        .store()
        .append_message(&NewMessage {
            session_id: session_id.to_string(),
            run_id: None,
            agent_id: None,
            role: "assistant".to_string(),
            content: content.clone(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: Some(name),
            images: Vec::new(),
            reasoning_content: None,
        })
        .await?;
    state.runtime().emit(
        Some(session_id),
        None,
        None,
        Event::CommandResult {
            command: command.to_string(),
            message_id,
            content,
            state: if reply.ok { "ok" } else { "error" }.to_string(),
        },
    );
    Ok(message_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(name: &str, args: &str) -> Invocation {
        Invocation {
            name: name.into(),
            args: args.into(),
        }
    }

    #[test]
    fn a_command_is_the_whole_first_token() {
        assert_eq!(
            parse("/compact").unwrap(),
            invocation("compact", ""),
            "a bare command carries no arguments"
        );
        assert_eq!(
            parse("/model gpt-4.1").unwrap(),
            invocation("model", "gpt-4.1")
        );
        assert_eq!(
            parse("/rename   two  spaces  ").unwrap(),
            invocation("rename", "two  spaces"),
            "arguments keep their inner spacing and lose the outer"
        );
        assert_eq!(
            parse("/HELP").unwrap().name,
            "help",
            "names are folded to lower case"
        );
    }

    #[test]
    fn a_message_that_merely_starts_with_a_slash_is_a_prompt() {
        // A path is the case this rule exists for.
        assert_eq!(parse("/home/toil/notes.md — why?"), None);
        assert_eq!(parse("// a comment"), None);
        assert_eq!(parse("/"), None);
        assert_eq!(parse("/ compact"), None);
        assert_eq!(parse(" /compact"), None, "a leading space is not a command");
        assert_eq!(parse("run /compact"), None);
        assert_eq!(
            parse("/compact!"),
            None,
            "punctuation ends the name, not the command"
        );
        assert_eq!(parse("/9lives"), None, "a name starts with a letter");
    }

    #[test]
    fn every_builtin_is_reachable_by_its_own_name() {
        for builtin in BUILTINS {
            let parsed = parse(&format!("/{}", builtin.name))
                .unwrap_or_else(|| panic!("/{} must parse", builtin.name));
            assert_eq!(parsed.name, builtin.name);
            assert!(is_builtin(builtin.name));
            assert!(
                is_command_token(builtin.name),
                "/{} must be typeable as a skill name too",
                builtin.name
            );
        }
    }

    #[test]
    fn the_menu_lists_every_builtin_once_and_says_how_to_insert_it() {
        let entries: Vec<Entry> = builtins().collect();
        assert_eq!(entries.len(), BUILTINS.len());
        let mut names = entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), BUILTINS.len(), "a command name is unique");
        for entry in &entries {
            assert!(entry.insert.starts_with('/'));
            assert_eq!(
                entry.insert.ends_with(' '),
                !entry.args.is_empty(),
                "{} must insert a trailing space exactly when it takes arguments",
                entry.name
            );
            assert!(
                !entry.summary.is_empty(),
                "{} needs a menu summary",
                entry.name
            );
            assert!(
                !entry.summary.ends_with('.'),
                "{}: summaries are fragments",
                entry.name
            );
        }
    }

    #[test]
    fn a_typo_suggests_the_command_it_was_meant_to_be() {
        let catalog: Vec<Entry> = builtins().collect();
        assert_eq!(suggestions(&catalog, "compcat"), vec!["compact"]);
        assert_eq!(suggestions(&catalog, "hlep"), vec!["help"]);
        assert_eq!(
            suggestions(&catalog, "renmae"),
            vec!["rename"],
            "a transposition is one edit away"
        );
        assert!(
            !suggestions(&catalog, "deploy").contains(&"docs".to_string()),
            "an unrelated word must not be guessed at"
        );
    }

    #[test]
    fn skill_ids_are_only_commands_when_they_look_like_one() {
        assert!(is_command_token("release"));
        assert!(is_command_token("fix-bug_2"));
        assert!(!is_command_token("Release"), "no capitals");
        assert!(!is_command_token("fix.bug"));
        assert!(!is_command_token("2fast"));
        assert!(!is_command_token(""));
    }

    #[test]
    fn a_reply_knows_whether_it_answers_or_rewrites() {
        assert!(Reply::ok("done").answers());
        assert!(Reply::fail("no").answers());
        let rewritten = Reply::run("a prompt");
        assert!(!rewritten.answers());
        assert_eq!(rewritten.prompt.as_deref(), Some("a prompt"));
        assert!(rewritten.ok);
    }
}
