//! The body of each slash command, and the markdown each one answers with.
//!
//! These are the implementations [`super::run`] dispatches to. They share one
//! shape - take the session, answer with a [`Reply`] - because the differences
//! between them are all in the answer, not in the plumbing.
//!
//! Two rules hold for every command here:
//!
//! * **A refusal is a result, not an error.** "A run is in flight, stop it first"
//!   is something the reader needs to see in the transcript, next to the command
//!   they typed; returning a `409` would put it in a toast that scrolls away. So
//!   a command that cannot do its job answers `ok: false` with the reason and is
//!   still recorded.
//! * **The answer is markdown, and it is short.** It ends up in a transcript that
//!   is already long; a command that dumps a tool schema or a whole skill body
//!   makes the conversation harder to read than not running it.

use super::{is_command_token, Entry, Invocation, Reply, Source};
use crate::error::{ApiError, ApiResult};
use crate::routes::sessions::MAX_TITLE_CHARS;
use crate::settings::validate_session_model;
use crate::state::AppState;
use kc_core::config::{Config, ModelConfig, SessionModelSelection, REASONING_EFFORTS};
use kc_core::events::Event;
use kc_core::paths::Workspace;
use kc_core::skill::{SkillRegistry, SkillSource, SkillsConfig};
use kc_core::store::Session;
use kc_core::tool::ToolOrigin;
use kc_core::Settings;
use std::sync::Arc;

/// How many catalog rows `/help` prints before it stops listing.
const HELP_MAX_ROWS: usize = 64;
/// How many tools per origin `/tools` and `/mcp` name.
const TOOL_LIST_MAX: usize = 40;
/// How many documentation matches `/docs` returns.
const DOCS_MAX_HITS: usize = 5;
/// How many subagents `/agents` lists.
const AGENTS_MAX_ROWS: usize = 30;

// ---------------------------------------------------------------------------
// /help
// ---------------------------------------------------------------------------

pub(super) async fn help(catalog: &[Entry], invocation: &Invocation) -> ApiResult<Reply> {
    if invocation.has_args() {
        let wanted = invocation.args_trimmed().trim_start_matches('/');
        let Some(entry) = super::find(catalog, wanted) else {
            let close = super::suggestions(catalog, wanted);
            return Ok(Reply::fail(format!(
                "There is no `/{wanted}` command.{}",
                did_you_mean(&close)
            )));
        };
        let mut text = format!(
            "### `/{name}`\n\n{title}.\n\n",
            name = entry.name,
            title = entry.title
        );
        text.push_str(&format!("{}\n\n", entry.summary));
        if !entry.args.is_empty() {
            text.push_str(&format!("**Arguments:** `{}`\n\n", entry.args));
        }
        text.push_str(&format!(
            "**From:** {}\n",
            match entry.source {
                Source::Builtin => "built in",
                Source::Skill => "a skill in this workspace",
            }
        ));
        return Ok(Reply::ok(text));
    }

    let mut text = String::from(
        "## Slash commands\n\n\
         Type one as the first word of a message. `/` opens this list in the message box.\n",
    );
    let mut category = None;
    for (index, entry) in catalog.iter().enumerate() {
        if index >= HELP_MAX_ROWS {
            text.push_str("\n_The list is cut here; `/help <command>` describes one._\n");
            break;
        }
        if category != Some(entry.category) {
            category = Some(entry.category);
            text.push_str(&format!("\n### {}\n\n", entry.category.title()));
            text.push_str("| Command | What it does |\n| --- | --- |\n");
        }
        let usage = if entry.args.is_empty() {
            format!("/{}", entry.name)
        } else {
            format!("/{} {}", entry.name, entry.args)
        };
        text.push_str(&format!(
            "| `{}` | {} |\n",
            escape(&usage),
            escape(&entry.summary)
        ));
    }
    text.push_str(
        "\nAnything that is not a command is an ordinary message - including a path that \
         starts with `/`.\n",
    );
    Ok(Reply::ok(text))
}

// ---------------------------------------------------------------------------
// /compact
// ---------------------------------------------------------------------------

pub(super) async fn compact(
    state: &AppState,
    session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    if invocation.has_args() {
        return Ok(Reply::fail(
            "`/compact` takes no arguments: it compacts this session's conversation as it stands.",
        ));
    }
    match state.runtime().compact_session(&session.id).await {
        Ok(report) if report.compacted => {
            let saved = report.chars_before.saturating_sub(report.chars_after);
            let mut text = format!(
                "### Compacted\n\n\
                 **{} → {} messages**, {} → {} characters ({} saved).\n\n\
                 The dropped turns are replaced by one digest that keeps the task, what was \
                 changed and what is still open. The next message in this session starts from \
                 that digest; nothing was deleted from the transcript.\n",
                report.messages_before,
                report.messages_after,
                thousands(report.chars_before),
                thousands(report.chars_after),
                thousands(saved),
            );
            if report.semantic {
                text.push_str(
                    "\nThe digest also carries a model-written summary of the work that \
                               was folded away.\n",
                );
            }
            Ok(Reply::ok(text))
        }
        Ok(report) => Ok(Reply::ok(format!(
            "### Nothing to compact\n\n\
             This conversation is already {} characters, inside the {} character target a \
             compaction would fold down to. Compacting now would rewrite the digest for no \
             saving and cost a cache miss.",
            thousands(report.chars_before),
            thousands(report.target_chars),
        ))),
        Err(kc_core::Error::BadRequest(message)) => Ok(Reply::fail(message)),
        Err(error) => Err(ApiError::from(error)),
    }
}

// ---------------------------------------------------------------------------
// /context
// ---------------------------------------------------------------------------

pub(super) async fn context(
    state: &AppState,
    session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    if invocation.has_args() {
        return Ok(Reply::fail("`/context` takes no arguments."));
    }
    let model = state.settings().resolve_model(session.model.as_ref()).await;
    let limits = state.settings().limits().await;
    let budget = limits.context_chars_for(&model);
    let stats = state.store().session_stats(&session.id).await?;
    let pinned = match &session.model {
        Some(selection) => format!(
            "pinned to `{}/{}`{}",
            selection.provider,
            selection.model,
            selection
                .reasoning_effort
                .as_deref()
                .map(|effort| format!(" at effort `{effort}`"))
                .unwrap_or_default()
        ),
        None => "following the install-wide default".to_string(),
    };
    let window = model
        .context_window
        .map(|tokens| format!("{} tokens", thousands(tokens as usize)))
        .unwrap_or_else(|| "not declared by the model".to_string());
    let text = format!(
        "### Context\n\n\
         | | |\n| --- | --- |\n\
         | Model | `{}` on `{}` ({}) |\n\
         | Window | {} |\n\
         | Request budget | {} characters |\n\
         | `/compact` target | {} characters |\n\
         | Messages | {} in this conversation, {} in subagents |\n\
         | Tools | {} available |\n\n\
         The budget is what one request may carry. `/compact` folds the conversation down to \
         a quarter of it; until then older turns ride along and the provider's prefix cache \
         keeps them cheap.\n",
        escape(&model.model),
        escape(&model.provider),
        pinned,
        window,
        thousands(budget),
        thousands((budget / 4).max(2_000)),
        thousands(stats.messages_main as usize),
        thousands(stats.messages_subagent as usize),
        state.runtime().tools().len(),
    );
    Ok(Reply::ok(text))
}

// ---------------------------------------------------------------------------
// /model
// ---------------------------------------------------------------------------

pub(super) async fn model(
    state: &AppState,
    session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    let config = state.settings().snapshot();
    if invocation.args_trimmed().eq_ignore_ascii_case("clear")
        || invocation.args_trimmed().eq_ignore_ascii_case("default")
    {
        state.store().set_session_model(&session.id, None).await?;
        state.runtime().emit(
            Some(&session.id),
            None,
            None,
            Event::SessionModelChanged {
                session_id: session.id.clone(),
                model: None,
            },
        );
        let active = state.settings().resolve_model(None).await;
        return Ok(Reply::ok(format!(
            "This session follows the install-wide default again: `{}/{}`.",
            active.provider, active.model
        )));
    }

    if !invocation.has_args() {
        let (provider, model) = match &session.model {
            Some(selection) => (selection.provider.clone(), selection.model.clone()),
            None => {
                let active = state.settings().resolve_model(None).await;
                (active.provider, active.model)
            }
        };
        let mut text = format!(
            "### Model\n\nThis session runs on `{}/{}`{}.\n\n",
            escape(&provider),
            escape(&model),
            if session.model.is_some() {
                " (pinned here)"
            } else {
                " (the install-wide default)"
            }
        );
        text.push_str("| Provider | Models |\n| --- | --- |\n");
        for profile in &config.providers {
            let models = if profile.models.is_empty() {
                "_any model id_".to_string()
            } else {
                profile
                    .models
                    .iter()
                    .map(|entry| {
                        let levels = entry
                            .variant_levels()
                            .filter(|levels| !levels.is_empty())
                            .map(|levels| format!(" ({})", levels.join("/")))
                            .unwrap_or_default();
                        format!("`{}{levels}`", entry.id)
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            text.push_str(&format!("| `{}` | {} |\n", escape(&profile.id), models));
        }
        text.push_str(
            "\n`/model <id>` pins this session, `/model <provider>/<model>` names both, and \
             `/model clear` goes back to the default.",
        );
        return Ok(Reply::ok(text));
    }

    let requested = invocation.args_trimmed();
    let selection = match resolve_selection(&config, session, requested, None) {
        Ok(selection) => selection,
        Err(message) => return Ok(Reply::fail(message)),
    };
    if let Err(error) = validate_session_model(&config, &selection) {
        return Ok(Reply::fail(format!(
            "`{requested}` cannot be used: {error}"
        )));
    }
    state
        .store()
        .set_session_model(&session.id, Some(&selection))
        .await?;
    state.runtime().emit(
        Some(&session.id),
        None,
        None,
        Event::SessionModelChanged {
            session_id: session.id.clone(),
            model: Some(selection.clone()),
        },
    );
    Ok(Reply::ok(format!(
        "Pinned this session to `{}/{}`{} - the next run uses it.",
        escape(&selection.provider),
        escape(&selection.model),
        selection
            .reasoning_effort
            .as_deref()
            .map(|effort| format!(" at effort `{effort}`"))
            .unwrap_or_default()
    )))
}

// ---------------------------------------------------------------------------
// /effort
// ---------------------------------------------------------------------------

pub(super) async fn effort(
    state: &AppState,
    session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    let config = state.settings().snapshot();
    let current = state.settings().resolve_model(session.model.as_ref()).await;
    let levels = model_levels(&config, &current);
    if !invocation.has_args() {
        let pinned_effort = session
            .model
            .as_ref()
            .and_then(|selection| selection.reasoning_effort.clone());
        let (effort, note) = match pinned_effort {
            Some(effort) => (format!("`{effort}`"), "pinned on this session".to_string()),
            None => (
                "the endpoint default".to_string(),
                "no effort is pinned, so the request carries none".to_string(),
            ),
        };
        return Ok(Reply::ok(format!(
            "### Reasoning effort\n\n\
             `{}/{}` runs at {effort} ({note}).\n\n\
             {}.\n\n\
             `/effort <level>` pins it for this session: {}.",
            escape(&current.provider),
            escape(&current.model),
            if levels.is_empty() {
                "This model declares no reasoning variants, so any level it does not implement \
                 would be ignored by the endpoint"
                    .to_string()
            } else {
                format!(
                    "This model offers {}",
                    levels
                        .iter()
                        .map(|level| format!("`{level}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
            REASONING_EFFORTS
                .iter()
                .map(|level| format!("`{level}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let requested = invocation.args_trimmed().to_ascii_lowercase();
    if !REASONING_EFFORTS.contains(&requested.as_str()) {
        return Ok(Reply::fail(format!(
            "`{requested}` is not a reasoning effort. Use one of {}.",
            REASONING_EFFORTS
                .iter()
                .map(|level| format!("`{level}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let selection = match resolve_selection(&config, session, "", Some(requested.clone())) {
        Ok(selection) => selection,
        Err(message) => return Ok(Reply::fail(message)),
    };
    if let Err(error) = validate_session_model(&config, &selection) {
        return Ok(Reply::fail(format!(
            "effort `{requested}` cannot be used with `{}/{}`: {error}",
            selection.provider, selection.model
        )));
    }
    state
        .store()
        .set_session_model(&session.id, Some(&selection))
        .await?;
    state.runtime().emit(
        Some(&session.id),
        None,
        None,
        Event::SessionModelChanged {
            session_id: session.id.clone(),
            model: Some(selection.clone()),
        },
    );
    Ok(Reply::ok(format!(
        "This session now runs `{}/{}` at effort `{requested}`.",
        escape(&selection.provider),
        escape(&selection.model)
    )))
}

// ---------------------------------------------------------------------------
// /rename
// ---------------------------------------------------------------------------

pub(super) async fn rename(
    state: &AppState,
    session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    let title = match invocation.required_arg("/rename <title>") {
        Ok(title) => title.trim(),
        Err(message) => return Ok(Reply::fail(message)),
    };
    if title.contains('\0') {
        return Ok(Reply::fail("A title may not contain NUL bytes."));
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Ok(Reply::fail(format!(
            "That title is {} characters; the limit is {MAX_TITLE_CHARS}.",
            title.chars().count()
        )));
    }
    state.store().rename_session(&session.id, title).await?;
    state.runtime().emit(
        Some(&session.id),
        None,
        None,
        Event::SessionRenamed {
            session_id: session.id.clone(),
            title: title.to_string(),
        },
    );
    Ok(Reply::ok(format!(
        "Renamed this session to **{}**.",
        escape(title)
    )))
}

// ---------------------------------------------------------------------------
// /stop
// ---------------------------------------------------------------------------

pub(super) async fn stop(
    state: &AppState,
    session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    if invocation.has_args() {
        return Ok(Reply::fail("`/stop` takes no arguments."));
    }
    if state.runtime().cancel(&session.id) {
        Ok(Reply::ok(
            "Stopped the run. Whatever it had already written stays in the transcript.",
        ))
    } else {
        Ok(Reply::fail("Nothing is running in this session."))
    }
}

// ---------------------------------------------------------------------------
// /skills and /skill
// ---------------------------------------------------------------------------

pub(super) async fn skills(
    state: &AppState,
    session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    if invocation.has_args() {
        return Ok(Reply::fail(
            "`/skills` takes no arguments; `/skill <name>` runs one.",
        ));
    }
    let config = state.settings().skills().await;
    if !config.enabled {
        return Ok(Reply::fail(
            "Skills are turned off (`[skills] enabled = false`), so there is nothing to list.",
        ));
    }
    let workspace = session_workspace(state, session)?;
    let registry = discover_skills(&config, &workspace).await?;
    Ok(Reply::ok(render_skills(&registry)))
}

pub(super) async fn skill(
    state: &AppState,
    session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    let arg = match invocation.required_arg("/skill <name> [task]") {
        Ok(arg) => arg,
        Err(_) => {
            // No name: show the library instead of an error - that is what the
            // reader is asking for, and it names the ids /skill needs.
            let config = state.settings().skills().await;
            if !config.enabled {
                return Ok(Reply::fail(
                    "Skills are turned off (`[skills] enabled = false`).",
                ));
            }
            let workspace = session_workspace(state, session)?;
            let registry = discover_skills(&config, &workspace).await?;
            return Ok(Reply::ok(render_skills(&registry)));
        }
    };
    let (name, task) = split_skill_arg(arg);
    skill_named(state, session, name, task).await
}

/// The `/skill` body, also used when a skill is invoked by its own name.
pub(super) async fn skill_named(
    state: &AppState,
    session: &Session,
    name: &str,
    task: &str,
) -> ApiResult<Reply> {
    let config = state.settings().skills().await;
    if !config.enabled {
        return Ok(Reply::fail(
            "Skills are turned off (`[skills] enabled = false`).",
        ));
    }
    let workspace = session_workspace(state, session)?;
    let registry = discover_skills(&config, &workspace).await?;
    let Some(skill) = registry.get(name) else {
        let known = registry
            .skills
            .iter()
            .map(|skill| skill.id.clone())
            .collect::<Vec<_>>();
        let close = super::suggestions(
            &known
                .iter()
                .map(|id| Entry {
                    name: id.clone(),
                    title: id.clone(),
                    summary: String::new(),
                    args: String::new(),
                    category: super::Category::Extend,
                    source: Source::Skill,
                    insert: format!("/{id}"),
                })
                .collect::<Vec<_>>(),
            name,
        );
        return Ok(Reply::fail(format!(
            "There is no `{name}` skill.{}",
            did_you_mean(&close)
        )));
    };
    if !skill.enabled {
        return Ok(Reply::fail(format!(
            "The `{}` skill is turned off. Turn it back on in Settings → Skills, or in \
             `[skills] disabled`.",
            skill.id
        )));
    }
    // The prompt this becomes is what the model reads, so it names the tool, the
    // action and the id: `/skill review` must work the same way in a fresh session
    // as it does in one where the skill was already read.
    let task = task.trim();
    let mut prompt = format!(
        "Use the `{}` skill. Read it with the `skill` tool \
         (`{{\"action\":\"read\",\"name\":\"{}\"}}`) and follow its procedure.",
        skill.id, skill.id
    );
    if task.is_empty() {
        prompt.push_str(&format!(
            "\n\nTask: apply `{}` to the work in front of us.",
            skill.id
        ));
    } else {
        prompt.push_str("\n\nTask: ");
        prompt.push_str(task);
    }
    Ok(Reply::run(prompt))
}

fn render_skills(registry: &SkillRegistry) -> String {
    if registry.skills.is_empty() {
        return "### Skills\n\nNo skills were found. Drop a `SKILL.md` in `.karucode/skills/<name>/` \
                (or add one in Settings → Skills) and it shows up here.\n"
            .to_string();
    }
    let mut text =
        String::from("### Skills\n\n| Skill | From | When to use it |\n| --- | --- | --- |\n");
    for skill in &registry.skills {
        let origin = match skill.source {
            SkillSource::Workspace => "this project",
            SkillSource::User => "your home folder",
            SkillSource::Config => "a configured folder",
            SkillSource::Custom => "added here",
        };
        let name = if is_command_token(&skill.id) && !skill.enabled {
            format!("`{}` (off)", escape(&skill.id))
        } else if is_command_token(&skill.id) {
            format!("`/{}`", escape(&skill.id))
        } else {
            format!("`{}`", escape(&skill.id))
        };
        text.push_str(&format!(
            "| {} | {} | {} |\n",
            name,
            origin,
            if skill.description.trim().is_empty() {
                "_no description_".to_string()
            } else {
                escape(&skill.description)
            }
        ));
    }
    text.push_str(
        "\nRun one with `/skill <name> <task>`, or with `/<name> <task>` when the name is a \
         command. The agent can also pick a skill itself.\n",
    );
    if registry.truncated {
        text.push_str("\n_The list was cut at `skills.max_skills`._\n");
    }
    text
}

/// Split `/skill <name> <task>` into the skill name and the rest.
fn split_skill_arg(arg: &str) -> (&str, &str) {
    let trimmed = arg.trim();
    match trimmed.find(char::is_whitespace) {
        Some(index) => (&trimmed[..index], trimmed[index..].trim()),
        None => (trimmed, ""),
    }
}

// ---------------------------------------------------------------------------
// /mcp and /tools
// ---------------------------------------------------------------------------

pub(super) async fn mcp(
    state: &AppState,
    _session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    if invocation.has_args() {
        return Ok(Reply::fail("`/mcp` takes no arguments."));
    }
    let mut servers: Vec<(String, Vec<String>)> = Vec::new();
    for spec in state.runtime().tools().specs() {
        let ToolOrigin::Mcp { server } = &spec.origin else {
            continue;
        };
        match servers.iter_mut().find(|(name, _)| name == server) {
            Some((_, tools)) => tools.push(spec.name),
            None => servers.push((server.clone(), vec![spec.name])),
        }
    }
    if servers.is_empty() {
        return Ok(Reply::ok(
            "### MCP\n\nNo MCP server contributes a tool right now. Add one to \
             `[[mcp_servers]]` in the config and run `/mcp` again after \
             `POST /api/mcp/reload` (a reload is also what picks up an edited entry).\n"
                .to_string(),
        ));
    }
    servers.sort_by(|a, b| a.0.cmp(&b.0));
    let mut text = String::from("### MCP\n\n| Server | Tools |\n| --- | --- |\n");
    for (server, tools) in &servers {
        text.push_str(&format!(
            "| `{}` | {} |\n",
            escape(server),
            name_list(tools)
        ));
    }
    text.push_str(
        "\nAn MCP tool is called `mcp__<server>__<tool>`. A server that is configured but \
         contributed nothing is not running - the plugin and MCP log says why.\n",
    );
    Ok(Reply::ok(text))
}

pub(super) async fn tools(
    state: &AppState,
    _session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    if invocation.has_args() {
        return Ok(Reply::fail("`/tools` takes no arguments."));
    }
    let specs = state.runtime().tools().specs();
    if specs.is_empty() {
        return Ok(Reply::fail(
            "No tool is registered; the agent can only talk.",
        ));
    }
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for spec in specs {
        let label = spec.origin.label();
        match groups.iter_mut().find(|(name, _)| *name == label) {
            Some((_, names)) => names.push(spec.name),
            None => groups.push((label, vec![spec.name])),
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    let total: usize = groups.iter().map(|(_, names)| names.len()).sum();
    let mut text = format!("### Tools\n\n{total} available.\n");
    for (label, names) in &groups {
        text.push_str(&format!(
            "\n**{}** ({}) - {}\n",
            match label.as_str() {
                "builtin" => "Built in",
                other => other,
            },
            names.len(),
            name_list(names)
        ));
    }
    text.push_str(
        "\nApprovals decide which of these may actually write: read-only tools never ask, and \
         the rest follow `[approvals] mode`.\n",
    );
    Ok(Reply::ok(text))
}

// ---------------------------------------------------------------------------
// /agents
// ---------------------------------------------------------------------------

pub(super) async fn agents(
    state: &AppState,
    session: &Session,
    invocation: &Invocation,
) -> ApiResult<Reply> {
    if invocation.has_args() {
        return Ok(Reply::fail("`/agents` takes no arguments."));
    }
    let records = state.store().list_agents(&session.id).await?;
    if records.is_empty() {
        return Ok(Reply::ok(
            "### Subagents\n\nNone. The agent spawns one with the `spawn_agent` tool when a \
             task can be delegated.\n",
        ));
    }
    let mut text = String::from(
        "### Subagents\n\n| Agent | Role | Status | Iterations |\n| --- | --- | --- | --- |\n",
    );
    for record in records.iter().take(AGENTS_MAX_ROWS) {
        text.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            escape(&record.name),
            escape(&record.role),
            escape(&record.status),
            record.iterations
        ));
    }
    if records.len() > AGENTS_MAX_ROWS {
        text.push_str(&format!(
            "\n_{} more in the Agents tab._\n",
            records.len() - AGENTS_MAX_ROWS
        ));
    }
    Ok(Reply::ok(text))
}

// ---------------------------------------------------------------------------
// /docs
// ---------------------------------------------------------------------------

pub(super) async fn docs(invocation: &Invocation) -> ApiResult<Reply> {
    let query = match invocation.required_arg("/docs <query>") {
        Ok(query) => query,
        Err(_) => {
            let mut text =
                String::from("### karucode documentation\n\n| Topic | About |\n| --- | --- |\n");
            for doc in kc_docs::all() {
                text.push_str(&format!(
                    "| `{}` | {} |\n",
                    escape(doc.slug),
                    escape(doc.summary)
                ));
            }
            text.push_str("\n`/docs <words>` searches these; the agent reads them with the `karucode_docs` tool.\n");
            return Ok(Reply::ok(text));
        }
    };
    let hits = kc_docs::search(query, DOCS_MAX_HITS);
    if hits.is_empty() {
        return Ok(Reply::fail(format!(
            "Nothing in karucode's documentation matches `{}`. Try a shorter phrase, or `/docs` \
             for the topic list.",
            escape(query)
        )));
    }
    let mut text = format!("### Documentation for `{}`\n", escape(query));
    for hit in hits {
        text.push_str(&format!(
            "\n**`{}`** - {}\n\n{}\n",
            escape(hit.doc.slug),
            escape(hit.doc.title),
            escape(&hit.snippet)
        ));
    }
    text.push_str("\n`/docs <topic>` prints a topic's summary; the agent reads the full page with the `karucode_docs` tool.\n");
    Ok(Reply::ok(text))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Discover skills the way `GET /api/skills` does.
///
/// Off the async runtime, because it is filesystem work over folders that can sit
/// on a slow mount, and forced **on** regardless of `[skills] enabled`: the master
/// switch decides whether the agent uses skills, not whether the menu can list
/// them.
pub async fn discover_skills(
    config: &SkillsConfig,
    workspace: &Workspace,
) -> ApiResult<SkillRegistry> {
    let config = SkillsConfig {
        enabled: true,
        ..config.clone()
    };
    let root = workspace.root().to_path_buf();
    tokio::task::spawn_blocking(move || kc_core::skill::discover(&config, &root))
        .await
        .map_err(|error| ApiError::internal(format!("the skills scan failed: {error}")))
}

fn session_workspace(state: &AppState, session: &Session) -> ApiResult<Arc<Workspace>> {
    state.workspace_for_session(session.project_id.as_deref(), &session.workspace)
}

/// Resolve `/model` and `/effort` arguments into a selection.
///
/// The empty `requested` means "keep the model and change only the effort", which
/// is what `/effort high` needs: an effort pin has to name a model, and inventing
/// one would silently move the session to the configuration default.
fn resolve_selection(
    config: &Config,
    session: &Session,
    requested: &str,
    effort: Option<String>,
) -> std::result::Result<SessionModelSelection, String> {
    let requested = requested.trim();
    let current = session.model.clone();
    let active = (&config.active_provider, &config.active_model);

    // "keep the model" - /effort, or an explicit empty argument.
    if requested.is_empty() {
        let Some(current) = current else {
            let provider = active
                .0
                .clone()
                .or_else(|| config.providers.first().map(|profile| profile.id.clone()))
                .ok_or_else(|| {
                    "no model provider is configured yet, so there is nothing to pin an effort to"
                        .to_string()
                })?;
            let model = active
                .1
                .clone()
                .unwrap_or_else(|| config.model.model.clone());
            if model.trim().is_empty() {
                return Err(format!(
                    "provider `{provider}` has no active model, so `/model <provider>/<model>` \
                     has to name one first"
                ));
            }
            return Ok(SessionModelSelection {
                provider,
                model,
                reasoning_effort: effort,
            }
            .normalized());
        };
        return Ok(SessionModelSelection {
            reasoning_effort: effort.or(current.reasoning_effort.clone()),
            ..current
        }
        .normalized());
    }

    // `<provider>/<model>`, when the prefix really is a profile id. A model id may
    // contain a slash of its own (`anthropic/claude-3`), which is why the split is
    // tried against the configured ids and not at the first slash unconditionally.
    if let Some((head, tail)) = requested.split_once('/') {
        if !tail.trim().is_empty() && config.providers.iter().any(|profile| profile.id == head) {
            return Ok(SessionModelSelection {
                provider: head.to_string(),
                model: tail.trim().to_string(),
                reasoning_effort: effort.or_else(|| current.and_then(|c| c.reasoning_effort)),
            }
            .normalized());
        }
    }

    // A bare model id: prefer the provider this session is already on, then the
    // install default, then the only provider that offers it.
    let effort = effort.or_else(|| current.as_ref().and_then(|c| c.reasoning_effort.clone()));
    let preferred = current
        .as_ref()
        .map(|selection| selection.provider.clone())
        .or_else(|| active.0.clone());
    if let Some(provider) = preferred {
        if let Some(profile) = config
            .providers
            .iter()
            .find(|profile| profile.id == provider)
        {
            if profile.models.is_empty() || profile.models.iter().any(|entry| entry.id == requested)
            {
                return Ok(SessionModelSelection {
                    provider,
                    model: requested.to_string(),
                    reasoning_effort: effort,
                }
                .normalized());
            }
        }
    }
    let offering = config
        .providers
        .iter()
        .filter(|profile| profile.models.iter().any(|entry| entry.id == requested))
        .map(|profile| profile.id.clone())
        .collect::<Vec<_>>();
    match offering.len() {
        0 => Err(format!(
            "`{requested}` is not a model of any configured provider. `/model` lists what each \
             one offers, or name both: `/model <provider>/<model>`."
        )),
        1 => Ok(SessionModelSelection {
            provider: offering[0].clone(),
            model: requested.to_string(),
            reasoning_effort: effort,
        }
        .normalized()),
        _ => Err(format!(
            "`{requested}` is offered by {} - name the provider: `/model <provider>/{requested}`.",
            offering
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The reasoning levels the model in front of us declares, or an empty list.
fn model_levels(config: &Config, model: &ModelConfig) -> Vec<&'static str> {
    config
        .providers
        .iter()
        .find(|profile| profile.id == model.provider)
        .and_then(|profile| profile.models.iter().find(|entry| entry.id == model.model))
        .and_then(|entry| entry.variant_levels())
        .unwrap_or_default()
}

/// `, `/x` and `/y`?` - the tail of an "unknown command" answer.
fn did_you_mean(names: &[String]) -> String {
    match names {
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
    }
}

/// A bounded list of names: `a`, `b` and *n* more.
fn name_list(names: &[String]) -> String {
    let shown = names
        .iter()
        .take(TOOL_LIST_MAX)
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > TOOL_LIST_MAX {
        format!("{shown} … and {} more", names.len() - TOOL_LIST_MAX)
    } else {
        shown
    }
}

/// Make text safe inside a markdown table cell.
fn escape(text: &str) -> String {
    text.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// `1234567` → `1,234,567`. Grouped by hand: the crate has no locale dependency
/// and a token count is read, not parsed.
fn thousands(value: usize) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_single_extra_word_is_taken_as_the_skill_name() {
        assert_eq!(split_skill_arg("release"), ("release", ""));
        assert_eq!(
            split_skill_arg("release  bump to 1.2 "),
            ("release", "bump to 1.2")
        );
        assert_eq!(
            split_skill_arg("review the diff in src/api"),
            ("review", "the diff in src/api")
        );
    }

    #[test]
    fn a_thousands_separator_groups_from_the_right() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(512_340), "512,340");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn a_table_cell_cannot_be_escaped_by_the_content_it_shows() {
        assert_eq!(escape("a|b"), "a\\|b");
        assert_eq!(escape("two\nlines"), "two lines");
    }

    #[test]
    fn an_unknown_name_suggests_at_most_three_commands() {
        assert_eq!(did_you_mean(&[]), "");
        assert_eq!(did_you_mean(&["help".into()]), " Did you mean `/help`?");
        assert_eq!(
            did_you_mean(&["help".into(), "skills".into()]),
            " Did you mean `/help` or `/skills`?"
        );
    }

    #[test]
    fn a_long_tool_list_names_how_many_it_did_not_show() {
        let names = (0..TOOL_LIST_MAX + 3)
            .map(|index| format!("tool{index}"))
            .collect::<Vec<_>>();
        let rendered = name_list(&names);
        assert!(rendered.ends_with("… and 3 more"), "{rendered}");
        assert!(rendered.starts_with("`tool0`"));
    }

    #[test]
    fn a_bare_model_id_resolves_inside_the_sessions_own_provider() {
        let config = Config {
            providers: vec![
                profile("env", &["small", "large"]),
                profile("other", &["small"]),
            ],
            active_provider: Some("env".into()),
            ..Default::default()
        };
        let session = session_with(None);
        let selection = resolve_selection(&config, &session, "large", None).unwrap();
        assert_eq!(selection.provider, "env");
        assert_eq!(selection.model, "large");

        let ambiguous = resolve_selection(&config, &session, "small", None);
        assert!(
            ambiguous.is_ok(),
            "the session's own provider wins over another one offering the same id"
        );

        let none = resolve_selection(&config, &session, "huge", None);
        assert!(
            none.unwrap_err().contains("not a model"),
            "unknown ids are refused"
        );
    }

    #[test]
    fn a_provider_prefixed_model_id_names_both_halves() {
        let config = Config {
            providers: vec![profile("env", &["small"]), profile("other", &["small"])],
            ..Default::default()
        };
        let session = session_with(None);
        let selection = resolve_selection(&config, &session, "other/small", None).unwrap();
        assert_eq!(selection.provider, "other");
        assert_eq!(selection.model, "small");
    }

    #[test]
    fn a_model_id_with_a_slash_of_its_own_is_not_split() {
        let config = Config {
            providers: vec![profile("env", &[])],
            active_provider: Some("env".into()),
            ..Default::default()
        };
        let session = session_with(None);
        let selection = resolve_selection(&config, &session, "anthropic/claude-3", None).unwrap();
        assert_eq!(
            selection.provider, "env",
            "`anthropic` is not a profile id, so the slash belongs to the model id"
        );
        assert_eq!(selection.model, "anthropic/claude-3");
    }

    #[test]
    fn an_effort_pin_keeps_the_model_it_is_on() {
        let config = Config {
            providers: vec![profile("env", &["small"])],
            ..Default::default()
        };
        let session = session_with(Some(SessionModelSelection {
            provider: "env".into(),
            model: "small".into(),
            reasoning_effort: None,
        }));
        let selection = resolve_selection(&config, &session, "", Some("high".into())).unwrap();
        assert_eq!(selection.model, "small");
        assert_eq!(selection.reasoning_effort.as_deref(), Some("high"));
    }

    fn profile(id: &str, models: &[&str]) -> kc_core::config::ProviderProfile {
        kc_core::config::ProviderProfile {
            id: id.into(),
            name: id.into(),
            models: models
                .iter()
                .map(|model| kc_core::config::ModelEntry {
                    id: (*model).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn session_with(model: Option<SessionModelSelection>) -> Session {
        Session {
            id: "s_1".into(),
            title: "t".into(),
            workspace: "/tmp".into(),
            project_id: None,
            model,
            worktrees: None,
            ephemeral: false,
            created_at: 0,
            updated_at: 0,
        }
    }
}
