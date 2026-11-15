//! Subagent spawning, delegation tools, concurrency limits and status reporting.
//!
//! The runtime registers a family of delegate tools into the tool view it
//! exposes. Every one of them runs a *nested* agent loop with:
//!
//! * a fresh message history (system prompt marked `is_subagent`, task as the
//!   user turn) persisted with the agent id so it never pollutes the parent
//!   conversation;
//! * turns bounded by `settings.subagents.max_iterations` (0 = no ceiling), with
//!   wall clock by
//!   `timeout_secs` (plus a hard backstop on top of the in-loop deadline);
//! * a tool set that excludes `spawn_agent` unless recursion is explicitly
//!   enabled *and* the depth budget allows it;
//! * approvals that keep the parent policy but carry the agent id;
//! * a concurrency limit of `settings.subagents.max_concurrent`.
//!
//! A subagent failure is reported as a failed tool result; it never fails the
//! parent run. Cancelling the parent cancels children through a child token.

// @kc-doc: subagents
// @kc-title: Delegate work to subagents
// @kc-summary: How spawn_agent, spawn_agents and the control tools work - blocking and background, sequential and parallel - and the limits they run under.
// @kc-tags: subagents, delegation, agents, parallel, background, spawn_agent, spawn_agents, send_message, long-running
// @kc-applies-to: tool:spawn_agent, tool:spawn_agents, tool:send_message, tool:wait_for_agents, tool:list_subagents, tool:agent_history, tool:pause_agents, tool:resume_agents, tool:kill_agents, tool:report_status, tool:subagent_worktrees, field:worktree, config:[subagents], config:subagents.max_concurrent, config:subagents.allow_background, config:subagents.max_background, config:subagents.wait_timeout_secs, config:subagents.max_iterations, config:subagents.timeout_secs, config:subagents.worktrees, env:KARUCODE_SUBAGENT_CONCURRENCY, env:KARUCODE_SUBAGENT_MAX_ITERATIONS, env:KARUCODE_SUBAGENT_TIMEOUT_SECS, env:KARUCODE_SUBAGENT_ALLOW_BACKGROUND, env:KARUCODE_SUBAGENT_MAX_BACKGROUND, env:KARUCODE_SUBAGENT_WAIT_TIMEOUT_SECS, env:KARUCODE_SUBAGENT_WORKTREES, env:KARUCODE_ALLOW_RECURSIVE_SUBAGENTS, route:GET /api/sessions/:id, route:PATCH /api/sessions/:id, route:GET /api/agents, route:GET /api/agents/:id, route:POST /api/agents/:id/pause, route:POST /api/agents/:id/resume, route:POST /api/agents/:id/cancel, route:PATCH /api/config, route:GET /api/config, event:agent_spawned, event:agent_progress, event:agent_paused, event:agent_message, event:agent_note, event:agent_finished, ui:AgentsTab
//
// The model can call `spawn_agent` with a self-contained task and a list of
// `owned_paths`. A subagent is a **nested, bounded agent loop**: it gets a fresh
// history (system prompt marked as subagent, the task as the user turn), its own
// run id and agent id, and the same tool set minus the delegate tools unless
// recursion is explicitly allowed.
//
// You do not configure a subagent per task: you write the task, and the agent
// decides. To steer it, ask for delegation explicitly - "use a subagent to map
// the call sites while you fix the parser".
//
// # Sequential and parallel
//
// | Call | Blocks? | Pattern |
// | --- | --- | --- |
// | `spawn_agent` | yes | one job, then use its answer |
// | `spawn_agent` + `background: true` | no | start it, keep working, collect later |
// | `spawn_agents` | yes | a fan-out that runs together and returns one batch |
// | `spawn_agents` + `background: true` | no | start a fan-out and keep working |
//
// The blocking forms are the cheap ones: the parent spends one tool call and gets
// the answer as the result. The background forms cost a second call
// (`wait_for_agents`) but let the parent do something useful meanwhile. A
// fan-out is the case that was previously impossible - three `spawn_agent` calls
// inside one turn execute in order, so they never actually overlapped on
// anything but provider latency.
//
// ```json
// {"name": "spawn_agents", "arguments": {"tasks": [
//   {"task": "map the call sites of parse()", "name": "mapper", "role": "mapper"},
//   {"task": "review the diff for panics", "name": "reviewer", "role": "reviewer"}
// ]}}
// ```
//
// # Talking to a live agent
//
// | Tool | What it does |
// | --- | --- |
// | `send_message` | deliver text to one or more agents (or a parent) as their next user turn |
// | `wait_for_agents` | block until named (or all) background agents settle, bounded by a timeout |
// | `list_subagents` | the live roster: status, iterations, unread mail, last reported note |
// | `agent_history` | one agent's transcript - the audit trail behind its summary |
// | `pause_agents` / `resume_agents` | park an agent between iterations, and let it go |
// | `kill_agents` | stop an agent and everything below it |
// | `report_status` | a subagent says what it is doing, for its parent and the roster |
//
// A message is delivered where a user turn belongs - at the top of the
// recipient's next iteration, before the request is built. It never interrupts
// the turn in flight, for the same reason a user steer does not: aborting a model
// mid-plan throws away work and can persist a tool call with no result. So
// `send_message` returns `queued`, not "they have read it".
//
// Pausing works the same way: the agent finishes its current turn, then parks.
// That is the only point where stopping is free. A paused agent still holds its
// subagent slot, and `kill_agents` still works on it - a paused agent is never
// unkillable. HTTP controls and the model's pause/resume tools share one per-agent
// serialization guard through the live flag and store transition, so opposing
// calls cannot leave the two states disagreeing. `report_status` updates only its
// advisory note/progress fields and cannot resurrect or rewind a control state.
//
// **Ownership.** An agent may pause, resume, kill and inspect the agents it
// spawned and everything *they* spawned. Messaging is open within the sender's
// session, including between siblings and upwards to the parent: a child reporting
// a blocker is the case this exists for. Explicit ids in another session/project
// are rejected as not live, even though the process-wide hub can see them. A child
// can therefore inject a turn into its parent's conversation, but never into an
// unrelated session. The injected turn is labelled with who sent it.
//
// Agent and top-level run mailboxes accept at most 32 messages. The 32nd message
// is queued; the 33rd returns `full` without emitting a queued event. Parent-run
// mail is scoped to the exact session and run generation. Once that run finishes,
// cleanup rejects an unwinding child's late message instead of creating a queue
// its successor could read or persist; the tool result says nothing was queued.
// A named, already-settled child passed to `wait_for_agents` is resolved from its
// persisted row (including its real summary), not mistaken for an unknown live id.
//
// # Limits
//
// ```toml
// [subagents]
// max_concurrent = 3       # loops at once; 0 = no ceiling, KARUCODE_SUBAGENT_CONCURRENCY
// max_background = 8       # live background agents per session; 0 = no ceiling
// allow_background = true  # KARUCODE_SUBAGENT_ALLOW_BACKGROUND
// wait_timeout_secs = 600  # default bound on wait_for_agents
// max_iterations = 0       # turns per subagent;  KARUCODE_SUBAGENT_MAX_ITERATIONS
// allow_recursive = false  # KARUCODE_ALLOW_RECURSIVE_SUBAGENTS
// max_depth = 1            # 0 = no delegation; at most 255
// timeout_secs = 14400     # 4h wall clock;      KARUCODE_SUBAGENT_TIMEOUT_SECS
// worktrees = false        # own git checkout per child; KARUCODE_SUBAGENT_WORKTREES
// ```
//
// Concurrency is enforced by a semaphore, iterations and wall clock per subagent,
// and a hard backstop on top of the in-loop deadline. Cancelling the parent cancels
// children through a child token. A spawn past a finite `max_concurrent` or
// `max_background` is **refused**, never queued: a model that says "go" and then
// sees nothing happen is worse than one that was told "no".
//
// # Give each subagent its own worktree
//
// Off by default. Turn it on when several subagents edit the same repository and
// you want each child's changes separated instead of interleaved in one tree:
//
// ```toml
// [subagents]
// worktrees = true   # KARUCODE_SUBAGENT_WORKTREES=1 also works
// ```
//
// You do not have to edit the file yourself. Ask the agent - "turn on worktrees
// for subagents" - and it changes the same setting through
// `PATCH /api/config` (`{"subagents": {"worktrees": true}}`) or by calling
// `subagent_worktrees` with `scope: "global"`; the settings screen under
// **Subagents** has the switch as well. It applies to the next spawn, no restart
// needed, and it is persisted.
//
// ## Three scopes: one round, one session, the whole install
//
// The install-wide key is the **default**, not the last word. There are three
// scopes and the narrowest one that is set wins:
//
// | Scope | How you ask | Where it lives |
// | --- | --- | --- |
// | this round | `"worktree": true` (or `false`) on one `spawn_agent`/`spawn_agents` call | nothing - the argument itself |
// | this session | `subagent_worktrees` with `mode: "on"/"off"/"inherit"`, or `PATCH /api/sessions/:id` `{"worktrees": ...}` | the session row (`sessions.worktrees`), so it survives a restart |
// | the install | `subagents.worktrees` | the config |
//
// So "use worktrees for this round of subagents" is a spawn argument, "use
// worktrees for the rest of our session" is the tool (or the inspector's
// **Session** tab, section *Subagent isolation*), and "disable worktrees globally"
// is `subagent_worktrees` with `scope: "global"` or the settings screen. In the
// runtime the order is literal: the call's `worktree` argument, else the
// session's `worktrees`, else `subagents.worktrees`. Any of them can say *off* as
// well as *on* - a session that opted out stays shared after the install turns
// isolation on.
//
// ```json
// {"name": "spawn_agents", "arguments": {"worktree": true, "tasks": [
//   {"task": "port the parser", "name": "parser"},
//   {"task": "port the printer", "name": "printer"}
// ]}}
// {"name": "subagent_worktrees", "arguments": {"mode": "on"}}
// {"name": "subagent_worktrees", "arguments": {"mode": "off", "scope": "global"}}
// ```
//
// The decision is made once per spawn, at the moment the child starts: a settings
// change never moves a running child, and the next one follows the new value. A
// per-call argument leaves no trace - it is not remembered as the session's
// setting. `subagent_worktrees` is the top-level agent's tool only: a subagent is
// never offered it, because neither its siblings' isolation nor the install
// default was delegated to it.
//
// **What a child gets.** A detached checkout at
// `<workspace>/.karucode/worktrees/<agent-id>`, and every path tool it calls -
// `read_file`, `write_file`, `edit_file`, `run_command`, the terminal, all of
// them - is confined to that directory. Two children can edit the same file
// without seeing each other, and the parent's working tree is not touched while
// they work.
//
// The checkout does **not** start at your last commit: it starts from your
// working tree as it is right now. Staged and unstaged changes to tracked files
// are in it, and untracked files (everything `git status` lists as new) are
// copied in. So a child sent to "finish the parser change I started" opens the
// file you were editing, not the committed version. Ignored files are not
// copied, and nested repositories/submodules are not mirrored - a child that
// needs a build can run one in its own tree.
//
// **Getting the work back.** Nothing is merged for you. When a child finishes:
//
// * changed nothing: its checkout is removed and no branch is created;
// * changed files: they are committed onto a local branch
//   `karucode/subagent/<agent-id>`, the checkout is removed, and the child's
//   summary says which branch and how many files.
//
// That summary line is in the parent's tool result, so the parent knows the work
// exists and where. Review and integrate it yourself:
//
// ```bash
// git diff <base>..karucode/subagent/<agent-id>   # <base> is quoted in the summary
// git merge karucode/subagent/<agent-id>          # or cherry-pick, or restore --staged
// git branch -D karucode/subagent/<agent-id>      # when you are done with it
// ```
//
// **When it refuses.** Only a git repository with at least one commit can support
// a worktree. In a plain directory, or a repo with no commits, the spawn is
// refused - it never falls back to the shared tree, because a child quietly
// running somewhere other than where you asked is the surprise this feature
// exists to avoid. The refusal names the scope that asked: `subagents.worktrees`
// for the install default, and for the other two it also says whether this
// spawn's `worktree` argument or this session's `subagent_worktrees` setting is
// the one to change.
//
// **What it costs.** One `git worktree add` and a full checkout per child, so a
// large repository makes spawns slower; the untracked-file copy adds to that.
// Combined with a wide `spawn_agents` fan-out this is real disk and time.
//
// # Unlimited subagents
//
// `max_concurrent = 0` and `max_background = 0` each mean **no ceiling**, which is
// how an install says "get out of the way": every spawn is granted a slot
// immediately, and a `spawn_agents` fan-out starts every child it was given. Both
// are needed for a wide fan-out, because the two limits apply on top of each
// other - a background batch reads `max_background` for its capacity check and
// `max_concurrent` for its slots. `max_depth = 255` removes the practical nesting
// limit (the field is a `u8`, so that is the deepest it can express).
//
// Nothing else has to change: `active` and `peak` are still counted, the roster and
// the stats stay honest, and a spawn is still bounded by `max_depth`,
// `timeout_secs` and the install's wall clock. What stops a runaway is cost, not a
// counter - every live loop is a provider stream and a transcript, so a model that
// fans out two hundred ways bills for two hundred loops. That is the trade the
// setting makes, and it is why the defaults are small.
//
// Zero does not mean the same thing for every field in this section:
// `max_depth = 0` forbids delegation outright and `max_iterations = 0` means no
// turn ceiling, so read each field's own rule rather than one rule for all.
//
// # A subagent runs as long as the task needs
//
// A delegated task is a whole piece of work, so a subagent is not interrupted by
// a counter: `max_iterations` defaults to `0`, meaning no turn ceiling, and the
// child keeps working in its own transcript until it reports back. Nothing is
// injected into that transcript to "resume" it - there is no segment boundary,
// no checkpoint turn and no refresh message.
//
// What ends a subagent instead:
//
// * `timeout_secs` - the wall clock for one delegated task (4h by default);
// * the loop guard - four turns in a row that only return answers the child
//   already has stop it, and its summary says it was looping; a turn of nothing
//   but waits (`terminal_read` on a running terminal, `wait_for_agents`,
//   `list_subagents`) does not count, so a child watching a build is not stopped
//   for waiting;
// * the same per-call rule and repetition detector a run gets;
// * the parent cancelling it, or `cancel`/`kill_agents`.
//
// A non-zero `max_iterations` is a hard stop: the child reports what it has and
// the parent sees a failed tool result rather than a silent hang.
//
// The subagent is told its real budget in its system prompt (turn ceiling if any,
// wall clock), so it plans against the numbers it actually gets.
//
// # What it inherits and what it does not
//
// Inherits: workspace, approval policy, settings, the session's model pin (the
// call's own `model` argument still overrides just that child). Does not inherit:
// the parent's conversation, its tool results, or its claims. Transcripts are
// persisted with the agent id so the parent chat stays clean; the UI shows
// progress and the final result per agent.
//
// # Watching a subagent
//
// Open the **Agents** tab in the inspector. It lists agents for the whole project
// (not only the open session), so a review fan-out is visible while you work
// somewhere else. The status chips - Running, Paused, Done, Failed, Cancelled,
// Interrupted - carry counts and filter the list; switching to **This session**
// narrows it to the working session. A running agent has pause and stop buttons.
//
// With an unbounded `max_concurrent` a single fan-out can leave hundreds of rows
// here, so the list renders the first 60 - live agents first - and offers **Show
// all**. The count above the list and the filter chips always describe the whole
// set, not the rendered page.
//
// Click an agent to open its drill-in view: the task it was given, its model and
// parent, its last reported note, and its live transcript - assistant text,
// thinking, each tool call with status and output, commands with exit codes, and
// approvals it is waiting on. The transcript keeps streaming while the agent runs
// and is backfilled from SQLite when you open an agent that finished before the
// page loaded. Older pages arrive as you scroll back (there is no load button).
//
// Each row's iteration count is persisted as the loop advances it, so reloading
// the page, pressing refresh or opening a second tab shows the real number for a
// running agent instead of `0`.
//
// The same data is available over HTTP: `GET /api/agents?project=<id>`,
// `GET /api/agents/:id`, and `POST /api/agents/:id/pause`, `/resume`, `/cancel`.
// The list is paginated - `?limit=` (default 100, max 2000), `?offset=`, or
// `?all=true` - and answers `total` beside `running`/`paused`, so a client can
// page through a fleet of a thousand agents instead of downloading all of them.
// All are documented in `docs/API.md`.
//
// # Failure handling
//
// A subagent failure is a failed tool result for the parent - it never fails the
// parent run. A timeout, a provider error and a cancelled task all come back with
// an explanatory message. A *background* agent has no such result to carry the
// failure, so it notifies and leaves it in its row and its summary.
//
// A wall clock that runs out is a stop, not an abandonment. When the deadline
// lands while a tool is still in flight, the runtime fires the cancellation that
// tool can observe, gives it the same grace a person's stop gets (half a second)
// so it can kill its process group or close what it opened, and only then drops
// it and writes the result row. That row says the run deadline - it never reads
// as "cancelled" - so a transcript read afterwards can tell "the run ran out of
// time" from "somebody pressed stop". The child still ends `failed` with a
// `timed out after Ns` message and its `agent_finished` event says the same
// thing; a top-level run that hits `limits.max_run_secs` goes through the same
// path and ends `limit_reached`, never `cancelled`.
//
// Cancellation is also safe while a spawn is still preparing its persisted task.
// If that preparation future is dropped after creating the running row but before
// the child loop exists, a preparation guard records the agent as `cancelled`,
// releases its path claims and emits the terminal event; no unsupervised running
// row or concurrency slot is left behind.
//
// # Gotchas
//
// * **Subagents share the parent's live workspace by default**, including dirty
//   and untracked files. Child edits are immediately visible and survive
//   completion; `owned_paths` provides advisory conflict reporting, not locking
//   or isolation. Turn on `subagents.worktrees` (next section) - or isolate just
//   one session or one round with `subagent_worktrees` / a spawn's `worktree`
//   argument - when two children would otherwise race on the same files.
// * Isolation is chosen at spawn time, from three places (call argument, session
//   override, install default). If you turn it off and children still get their
//   own checkout, the session has an override: `subagent_worktrees`
//   `{"mode": "inherit"}` clears it, and the refusal text names the scope.
// * Recursion is off by default; enabling it without a depth budget turns one
//   question into an expensive fan-out.
// * Subagent runs cost tokens and time. Delegating a one-file read is slower than
//   reading the file.
// * Tool calls inside a turn execute in order, so a blocking fan-out still issues
//   its children one after another; `spawn_agents` is what makes them overlap.
// * `max_background` caps *live background* agents per session, when it is
//   finite. A top-level blocking spawn may wait for `max_concurrent` capacity.
//   Background and nested blocking spawns are refused immediately at capacity;
//   a nested waiter would otherwise deadlock while holding its ancestor's slot.
// * Setting `max_concurrent` to `0` does not make spawning free: it removes one
//   counter. `max_depth`, `timeout_secs`, the install's run clock and the bill for
//   every concurrent provider stream still apply.
// * A background agent outlives the turn that started it but not the run: when
//   the run ends its children are cancelled, and a restart marks them
//   `interrupted`.
// * A subagent runs on its session's pinned model, not the install-wide default:
//   pinning a session to a small model makes its subagents cheap too.
// * The `model` argument overrides that for one child. A valid id is one this
//   install created and made available: a model **any** provider profile
//   declares, not only the session's - and when the model lives on another
//   profile, the child runs on that profile's endpoint and per-model metadata
//   rather than the session's. The id is resolved before the child starts; an
//   id no profile offers (`sonnet`, `gpt-5`, or - the common mistake - the
//   *name* of the agent you are spawning) is refused as a tool error naming
//   real ids, so you can fix it in the same turn. Omit `model` to inherit the
//   session's. The check needs a catalog: an install that declares no models
//   anywhere refuses nothing, because it says nothing about which other ids
//   its endpoints accept.
// * A subagent that keeps working without progress burns tokens until
//   `timeout_secs`. Keep it pointed at one task; split unrelated work into
//   separate subagents instead of widening one.
// * `report_status` is a claim, not a measurement. Nothing verifies a progress
//   number, so treat it as a hint about what to look at.
// * A child's report reaches the parent cut at 8,000 bytes, and the cut marker
//   says what continues it: the rest of the child is in its transcript, which
//   `agent_history` returns (page older messages with `before`). Asking the child
//   to repeat a report that was only cut in transit costs a whole extra run.
//
// Full reference: `docs/internal/subagents.md` (this page) and the
// `subagents` topic; `GET /api/sessions/:id` returns the session's `agents`
// array alongside its messages and runs.

use crate::agent::{LoopOutcome, LoopSpec};
use crate::approvals::RuntimeApprovalGate;
use crate::events::now_ms;
use crate::hub::{AgentMessage, ControlError, LiveAgent, MailboxDelivery, MAX_MAILBOX};
use crate::RuntimeInner;
use async_trait::async_trait;
use futures_util::FutureExt;
use kc_core::config::ModelOverrideJudgement;
use kc_core::error::Result;
use kc_core::events::Event;
use kc_core::ids::AgentId;
use kc_core::message::Message;
use kc_core::store::{AgentRecord, AgentStatus, MessageScope, NewMessage, RunStatus};
use kc_core::tool::{
    truncate_output, truncate_output_with_advice, Tool, ToolCallRequest, ToolClass, ToolCtx,
    ToolOutcome, ToolRegistry, ToolSpec,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Name of the built-in delegate tool.
pub const SPAWN_AGENT_TOOL: &str = "spawn_agent";
/// Fan-out: several independent tasks, started together.
pub const SPAWN_AGENTS_TOOL: &str = "spawn_agents";
/// Deliver a message to a live agent (or to this agent's parent).
pub const SEND_MESSAGE_TOOL: &str = "send_message";
/// Block until background agents settle.
pub const WAIT_FOR_AGENTS_TOOL: &str = "wait_for_agents";
/// The live roster.
pub const LIST_SUBAGENTS_TOOL: &str = "list_subagents";
/// One agent's transcript.
pub const AGENT_HISTORY_TOOL: &str = "agent_history";
pub const PAUSE_AGENTS_TOOL: &str = "pause_agents";
pub const RESUME_AGENTS_TOOL: &str = "resume_agents";
pub const KILL_AGENTS_TOOL: &str = "kill_agents";
/// A subagent reports what it is doing, for its parent and the roster.
pub const REPORT_STATUS_TOOL: &str = "report_status";
/// Turn git worktrees for subagents on or off, for this session or the install.
pub const SUBAGENT_WORKTREES_TOOL: &str = "subagent_worktrees";

/// The delegate tools a child keeps at *every* depth, even with recursion off.
///
/// They are coordination, not multiplication: a child that cannot report status
/// is invisible to its parent, and one that cannot message is unable to say it is
/// blocked - which is the whole reason a hierarchy is worth having.
pub const COORDINATION_TOOLS: &[&str] = &[
    SEND_MESSAGE_TOOL,
    LIST_SUBAGENTS_TOOL,
    AGENT_HISTORY_TOOL,
    REPORT_STATUS_TOOL,
];

/// The delegate tools that only a child allowed to recurse may use: spawning more
/// work, and stopping or parking somebody else's.
pub const DELEGATION_ONLY_TOOLS: &[&str] = &[
    SPAWN_AGENT_TOOL,
    SPAWN_AGENTS_TOOL,
    WAIT_FOR_AGENTS_TOOL,
    PAUSE_AGENTS_TOOL,
    RESUME_AGENTS_TOOL,
    KILL_AGENTS_TOOL,
];

/// Every tool this module installs.
pub const DELEGATE_TOOLS: &[&str] = &[
    SPAWN_AGENT_TOOL,
    SPAWN_AGENTS_TOOL,
    SEND_MESSAGE_TOOL,
    WAIT_FOR_AGENTS_TOOL,
    LIST_SUBAGENTS_TOOL,
    AGENT_HISTORY_TOOL,
    PAUSE_AGENTS_TOOL,
    RESUME_AGENTS_TOOL,
    KILL_AGENTS_TOOL,
    REPORT_STATUS_TOOL,
    SUBAGENT_WORKTREES_TOOL,
];

/// How long a timed-out or cancelled subagent's tools get to observe the
/// cancellation token and kill their process groups before the loop future is
/// dropped. Dropping first would only kill the leader process (`kill_on_drop`),
/// leaving children behind.
const SUBAGENT_CANCEL_GRACE: Duration = Duration::from_millis(500);

/// How long a **background** child gets to record a run-end cancellation as
/// `cancelled` before its loop future is dropped and the status is written as
/// `interrupted` instead.
///
/// Longer than [`SUBAGENT_CANCEL_GRACE`] because this grace is on a *run's* exit
/// path rather than inside the child's own cleanup: a tool that honours the token
/// needs time to kill its process group and return, and declaring a live child
/// `interrupted` while it is still unwinding would make the roster lie in the
/// other direction.
pub(crate) const RUN_END_CHILD_GRACE: Duration = Duration::from_millis(1_000);

/// Upper bound for a subagent summary returned to the parent and stored on the
/// agent record.
pub const MAX_SUMMARY_BYTES: usize = 8_000;

/// What a cut report tells the parent instead of the generic advice.
///
/// [`truncate_output`]'s own marker names `limits.max_tool_output_bytes`, which is
/// *not* the bound that cut a report: a parent that raised it would still get
/// 8,000 bytes back. The bound is [`MAX_SUMMARY_BYTES`] and the call that reads
/// the rest of the child is `agent_history`, so that is what the marker says.
const SUMMARY_CUT_ADVICE: &str =
    "the report is capped at 8 000 bytes; read the rest of the child with `agent_history`";

/// Upper bound on `agent_history` output, so auditing one agent cannot eat a
/// parent's context in a single call.
const MAX_HISTORY_BYTES: usize = 24_000;

/// Counting limiter for concurrent subagents.
///
/// The limit is read from live settings on every acquire call, so a settings
/// change applies to the next spawn without restarting the runtime.
pub struct SubagentLimiter {
    pub config: kc_core::config::SubagentConfig,
    active: Mutex<usize>,
    peak: AtomicUsize,
    notify: Notify,
}

impl SubagentLimiter {
    pub fn new(config: kc_core::config::SubagentConfig) -> Self {
        Self {
            config,
            active: Mutex::new(0),
            peak: AtomicUsize::new(0),
            notify: Notify::new(),
        }
    }

    pub fn active(&self) -> usize {
        *self.active.lock().expect("subagent limiter")
    }

    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    /// Take a slot only if one is free right now.
    ///
    /// A background spawn must never park its parent's turn, so it either gets a
    /// slot immediately or is refused with an explanation. Waiting would turn a
    /// tool call into a sleep the model cannot see or break out of.
    ///
    /// `max == 0` is the documented "no ceiling": the slot is always granted, and
    /// `active`/`peak` are still counted so the stats stay honest. Note this is
    /// the *only* meaning 0 can have here - a reader who expected 0 to mean
    /// "allow nothing" would turn every spawn into a refusal.
    pub fn try_acquire(self: &Arc<Self>, max: usize) -> Option<SubagentPermit> {
        let mut active = self.active.lock().expect("subagent limiter");
        if !slot_available(*active, max) {
            return None;
        }
        *active += 1;
        self.peak.fetch_max(*active, Ordering::Relaxed);
        Some(SubagentPermit {
            limiter: Arc::clone(self),
        })
    }

    /// Wait until a slot is free. Returns `None` if `cancel` fires first.
    pub async fn acquire(
        self: &Arc<Self>,
        max: usize,
        cancel: &CancellationToken,
    ) -> Option<SubagentPermit> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut active = self.active.lock().expect("subagent limiter");
                if slot_available(*active, max) {
                    *active += 1;
                    self.peak.fetch_max(*active, Ordering::Relaxed);
                    return Some(SubagentPermit {
                        limiter: Arc::clone(self),
                    });
                }
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return None,
                _ = notified => {}
            }
        }
    }
}

/// Is a slot free under a ceiling of `max`?
///
/// `0` is "no ceiling", which is why this is a named rule rather than an inline
/// `active < max`: the saturating form would make 0 mean "allow nothing", the
/// exact opposite of what the docs promise.
fn slot_available(active: usize, max: usize) -> bool {
    max == 0 || active < max
}

/// Holds one concurrency slot; released on drop, even on early return.
pub struct SubagentPermit {
    limiter: Arc<SubagentLimiter>,
}

impl Drop for SubagentPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.limiter.active.lock() {
            *active = active.saturating_sub(1);
        }
        self.limiter.notify.notify_waiters();
    }
}

/// Cancel every subagent loop a **top-level run** started, and wait (bounded) for
/// their statuses to be recorded.
///
/// This is the code behind the documented rule (`subagent.rs` module doc,
/// `docs/internal/subagents.md`): *a background agent outlives the turn that
/// started it but not the run*. Every exit path of a top-level run reaches it,
/// because completed, failed, cancelled and panicked all leave `run_top_level`.
///
/// `parent_run_id` is the top-level run's id, which every child it spawned carries
/// as its record's `parent_run_id` - including children of children, since that
/// field is inherited. The whole subtree is therefore reaped by one filter, which
/// is the correct scope: nothing a run started may outlive it.
///
/// A *detached* child is a legitimate feature while the run is alive - it keeps
/// working, the parent can `wait_for_agents` on it, and the composer stays usable
/// because `has_active_run` ignores it. Only the run's end reaps it.
///
/// The status the child records is its own: a loop that honours the token reports
/// `cancelled` (via [`crate::subagent`]'s `Exit::ParentCancelled` arm), and one
/// that ignores it keeps working - it still holds a cancelled token, so
/// [`Runtime::is_running`] already reports the session as idle and the next thing
/// that touches it will see a loop that is unwinding. This function does not
/// overwrite a status another writer owns.
pub(crate) async fn cancel_run_children(inner: &Arc<RuntimeInner>, parent_run_id: &str) {
    let reaped: Vec<(String, Arc<LiveAgent>)> = inner
        .hub
        .list_all()
        .into_iter()
        .filter(|agent| agent.run_id == parent_run_id)
        .map(|agent| (agent.id.0.clone(), agent))
        .collect();
    if reaped.is_empty() {
        return;
    }
    let ids: Vec<String> = reaped.iter().map(|(id, _)| id.clone()).collect();
    let session_id = reaped[0].1.session_id.clone();
    // `kill` marks each one first, so a loop that observes the token immediately
    // still knows a control request stopped it rather than a crash. The status it
    // writes then is `cancelled`, which is the truth: the run's end asked it to
    // stop.
    let killed = inner.hub.kill(&session_id, &ids, "run ended");
    tracing::info!(
        parent_run = %parent_run_id,
        agents = killed.len(),
        "cancelling subagents when the run ended"
    );

    // Wait (bounded, and all at once) for each loop to write its own final
    // status, so a client that reacts to the run's `RunFinished` by refetching the
    // roster already sees `cancelled`. Concurrent because a run may have spawned
    // several agents and a fan-out must cost one grace, not one per child. A loop
    // that ignores the token past the grace is left registered: it will record
    // itself when it unwinds, and inventing a status here would race that write
    // with a worse one.
    let waiting = reaped.into_iter().map(|(_, live)| async move {
        let _ = crate::hub::wait_for_settled(&live, RUN_END_CHILD_GRACE, &CancellationToken::new())
            .await;
    });
    futures_util::future::join_all(waiting).await;
}

/// Install every delegate tool into `registry`.
pub(crate) fn install_delegate_tools(inner: &Arc<RuntimeInner>, registry: &mut ToolRegistry) {
    let weak = Arc::downgrade(inner);
    for tool in [
        Arc::new(SpawnAgentTool {
            inner: weak.clone(),
            fan_out: false,
        }) as Arc<dyn Tool>,
        Arc::new(SpawnAgentTool {
            inner: weak.clone(),
            fan_out: true,
        }),
        Arc::new(SendMessageTool {
            inner: weak.clone(),
        }),
        Arc::new(WaitForAgentsTool {
            inner: weak.clone(),
        }),
        Arc::new(ListSubagentsTool {
            inner: weak.clone(),
        }),
        Arc::new(AgentHistoryTool {
            inner: weak.clone(),
        }),
        Arc::new(PauseAgentsTool {
            inner: weak.clone(),
            resume: false,
        }),
        Arc::new(PauseAgentsTool {
            inner: weak.clone(),
            resume: true,
        }),
        Arc::new(KillAgentsTool {
            inner: weak.clone(),
        }),
        Arc::new(ReportStatusTool {
            inner: weak.clone(),
        }),
        Arc::new(SubagentWorktreesTool { inner: weak }),
    ] {
        registry.register(tool);
    }
}

// ---------------------------------------------------------------------------
// spawn
// ---------------------------------------------------------------------------

/// One task inside a `spawn_agent` or `spawn_agents` call.
#[derive(Debug, Clone)]
struct SpawnRequest {
    task: String,
    name: Option<String>,
    role: String,
    owned_paths: Vec<String>,
    model: Option<String>,
}

impl SpawnRequest {
    /// How a refusal refers to this task: its name if it has one, else its
    /// position in the batch - deliberately never the instruction text. A refusal
    /// becomes a tool result, and echoing the caller's own words back into the
    /// transcript is noise the caller already has.
    fn label(&self, index: usize) -> String {
        match &self.name {
            Some(name) => format!("`{name}`"),
            None => format!("task {}", index + 1),
        }
    }
}

/// What the caller resolved out of the tool arguments.
#[derive(Debug, Clone)]
struct SpawnOptions {
    background: bool,
    /// Wait for a background fan-out before returning, so the whole batch is one
    /// result. Only meaningful with `background: true`.
    wait: bool,
    /// This call's own isolation choice. `None` leaves the decision to the
    /// session override and then the install default; `Some(true)`/`Some(false)`
    /// pin it for exactly this batch, which is what "use worktrees for this
    /// round of subagents" asks for.
    worktree: Option<bool>,
}

/// Where a spawn's isolation decision came from, most specific first.
///
/// It is carried into the child so a refusal can name the switch the caller can
/// actually change: "subagents.worktrees is on" is unhelpful advice to somebody
/// who never touched that key and asked for isolation in one conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorktreeSource {
    /// The `worktree` argument of this spawn call.
    Call,
    /// This session's `subagent_worktrees` setting.
    Session,
    /// `subagents.worktrees`.
    Global,
}

impl WorktreeSource {
    /// How the refusal text names this source, or `None` for the install-wide
    /// setting the underlying message already names.
    fn refusal_note(self) -> Option<&'static str> {
        match self {
            Self::Call => Some(
                "this spawn asked for a worktree with `worktree: true`; drop the argument, or \
                 turn the setting off, to let subagents share the parent workspace",
            ),
            Self::Session => Some(
                "this session's `subagent_worktrees` setting asks for worktrees; clear it with \
                 `subagent_worktrees` `{\"mode\": \"inherit\"}` to let subagents share the parent \
                 workspace",
            ),
            Self::Global => None,
        }
    }
}

/// One spawn's resolved isolation: *where* the child runs and *which* scope
/// decided it.
///
/// The two travel together because the second is only ever read when the first is
/// `true` and the checkout could not be created: the refusal has to name the
/// switch the caller can change, and "subagents.worktrees is on" is wrong advice
/// to somebody who asked for isolation in one conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorktreePlan {
    on: bool,
    source: WorktreeSource,
}

/// Resolve one spawn's isolation: the call's own argument first, then the
/// session override, then `subagents.worktrees`.
///
/// The order is the whole point of the feature. A per-call `worktree` is the
/// narrowest statement ("this round") and wins outright, a session override is
/// the next ("the rest of this conversation"), and the install setting is the
/// fallback everything used to rely on.
fn resolve_worktrees(call: Option<bool>, session: Option<bool>, global: bool) -> WorktreePlan {
    match (call, session) {
        (Some(on), _) => WorktreePlan {
            on,
            source: WorktreeSource::Call,
        },
        (None, Some(on)) => WorktreePlan {
            on,
            source: WorktreeSource::Session,
        },
        (None, None) => WorktreePlan {
            on: global,
            source: WorktreeSource::Global,
        },
    }
}

pub(crate) struct SpawnAgentTool {
    inner: Weak<RuntimeInner>,
    /// `true` for `spawn_agents`, which starts several children at once.
    fan_out: bool,
}

fn spawn_spec(fan_out: bool) -> ToolSpec {
    if fan_out {
        ToolSpec {
            name: SPAWN_AGENTS_TOOL.to_string(),
            description: "Spawn several subagents that work **at the same time** on independent \
                          self-contained tasks in the same workspace, then return their summaries \
                          as one batch. Use this instead of several `spawn_agent` calls when the \
                          tasks do not depend on each other: separate `spawn_agent` calls inside \
                          one turn run one after another. Set `background: true` to return \
                          immediately and collect the results with `wait_for_agents`."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "minItems": 1,
                        "description": "One independent task per subagent. They run concurrently.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "task": {
                                    "type": "string",
                                    "description": "Self-contained instruction: what to inspect or change, and what to report back."
                                },
                                "name": { "type": "string", "description": "Short display name, e.g. `reviewer`." },
                                "role": { "type": "string", "description": "Role label shown in the UI." },
                                "owned_paths": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Workspace paths this subagent intends to modify."
                                },
                                "model": { "type": "string", "description": "Optional model override for this subagent: one of this install's model ids (any provider profile's catalog). Omit to inherit the session's model." }
                            },
                            "required": ["task"],
                            "additionalProperties": false
                        }
                    },
                    "background": {
                        "type": "boolean",
                        "description": "Start them and return immediately; collect with `wait_for_agents`."
                    },
                    "wait": {
                        "type": "boolean",
                        "description": "With `background: true`, still wait for the whole batch and return its summaries."
                    },
                    "worktree": {
                        "type": "boolean",
                        "description": "Run these subagents in their own git worktrees for this call only: true isolates them, false keeps them in the shared tree. Omit to follow this session's `subagent_worktrees` setting and then `subagents.worktrees`."
                    }
                },
                "required": ["tasks"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    } else {
        ToolSpec {
            name: SPAWN_AGENT_TOOL.to_string(),
            description: "Spawn a focused subagent that works on one self-contained task in the \
                          same workspace and reports back a summary. Use it to parallelise \
                          independent investigations or reviews. The subagent gets its own \
                          conversation, cannot see this conversation, and cannot ask you \
                          questions. Pass `background: true` to return immediately and keep \
                          working while it runs (and to run several at once)."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Self-contained instruction: what to inspect or change, and what to report back."
                    },
                    "name": { "type": "string", "description": "Short display name, e.g. `reviewer`." },
                    "role": { "type": "string", "description": "Role label shown in the UI, e.g. `reviewer`." },
                    "owned_paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Workspace paths this subagent intends to modify; used to warn about overlaps."
                    },
                    "model": { "type": "string", "description": "Optional model override for this subagent: one of this install's model ids (any provider profile's catalog). Omit to inherit the session's model." },
                    "background": {
                        "type": "boolean",
                        "description": "Return immediately instead of waiting for the subagent; collect the result with `wait_for_agents`."
                    },
                    "worktree": {
                        "type": "boolean",
                        "description": "Run this subagent in its own git worktree for this call only: true isolates it, false keeps it in the shared tree. Omit to follow this session's `subagent_worktrees` setting and then `subagents.worktrees`."
                    }
                },
                "required": ["task"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn spec(&self) -> ToolSpec {
        spawn_spec(self.fan_out)
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down; cannot spawn a subagent");
        };
        let result = if self.fan_out {
            run_fan_out(&inner, &request, &ctx).await
        } else {
            run_single_spawn(&inner, &request, &ctx).await
        };
        match result {
            Ok(outcome) => outcome,
            Err(e) => ToolOutcome::err(format!("spawn_agent failed: {e}")),
        }
    }
}

/// Parse one task object out of a `tasks` entry.
fn parse_task(value: &serde_json::Value) -> Result<SpawnRequest> {
    let obj = value.as_object().ok_or_else(|| {
        kc_core::Error::BadRequest("each entry of `tasks` must be an object".into())
    })?;
    let task = obj
        .get("task")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            kc_core::Error::BadRequest("each entry of `tasks` needs a non-empty `task`".into())
        })?
        .to_string();
    let owned_paths: Vec<String> = match obj.get("owned_paths") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(value) => serde_json::from_value(value.clone()).map_err(|_| {
            kc_core::Error::BadRequest("`owned_paths` must be an array of strings".into())
        })?,
    };
    let text = |key: &str| {
        obj.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Ok(SpawnRequest {
        task,
        name: text("name"),
        role: text("role").unwrap_or_else(|| "worker".to_string()),
        owned_paths,
        model: text("model"),
    })
}

fn parse_spawn_options(request: &ToolCallRequest) -> Result<SpawnOptions> {
    let flag = |key: &str| match request.arguments.get(key) {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(serde_json::Value::Bool(b)) => Ok(*b),
        Some(_) => Err(kc_core::Error::BadRequest(format!(
            "`{key}` must be a boolean"
        ))),
    };
    // A tri-state rather than a flag: absent is "whoever is above me decides",
    // and `null` means the same thing so a client that always sends the key does
    // not accidentally pin every spawn.
    let worktree = match request.arguments.get("worktree") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Bool(b)) => Some(*b),
        Some(_) => {
            return Err(kc_core::Error::BadRequest(
                "`worktree` must be a boolean or null".into(),
            ))
        }
    };
    Ok(SpawnOptions {
        background: flag("background")?,
        wait: flag("wait")?,
        worktree,
    })
}

async fn run_single_spawn(
    inner: &Arc<RuntimeInner>,
    request: &ToolCallRequest,
    ctx: &ToolCtx,
) -> Result<ToolOutcome> {
    let task = request
        .opt_str("task")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| kc_core::Error::BadRequest("missing non-empty `task` argument".into()))?;
    let role = request
        .opt_str("role")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "worker".to_string());
    let owned_paths: Vec<String> = match request.arguments.get("owned_paths") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(value) => serde_json::from_value::<Vec<String>>(value.clone()).map_err(|_| {
            kc_core::Error::BadRequest("`owned_paths` must be an array of strings".into())
        })?,
    };
    let spawn = SpawnRequest {
        task,
        name: request
            .opt_str("name")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        role,
        owned_paths,
        model: request
            .opt_str("model")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    };
    let options = parse_spawn_options(request)?;
    spawn_batch(inner, ctx, vec![spawn], &options, false).await
}

async fn run_fan_out(
    inner: &Arc<RuntimeInner>,
    request: &ToolCallRequest,
    ctx: &ToolCtx,
) -> Result<ToolOutcome> {
    let raw =
        request.arguments.get("tasks").cloned().ok_or_else(|| {
            kc_core::Error::BadRequest("`spawn_agents` needs a `tasks` array".into())
        })?;
    let items = raw
        .as_array()
        .ok_or_else(|| kc_core::Error::BadRequest("`tasks` must be an array".into()))?;
    if items.is_empty() {
        return Ok(ToolOutcome::err(
            "`tasks` is empty; call `spawn_agent` for a single task, or name the work to fan out",
        ));
    }
    let mut tasks = Vec::with_capacity(items.len());
    for item in items {
        match parse_task(item) {
            Ok(task) => tasks.push(task),
            Err(e) => return Ok(ToolOutcome::err(format!("invalid `tasks` entry: {e}"))),
        }
    }
    let options = parse_spawn_options(request)?;
    spawn_batch(inner, ctx, tasks, &options, true).await
}

/// Start every task in `tasks`, then either wait for all of them or return their
/// ids.
///
/// The children are started in order but they *run* together: each spawn inserts
/// its live entry and its permit before the batch waits, and the batch waits on
/// the whole set at the end. That is the difference between this and several
/// `spawn_agent` calls, which the loop executes one after the other.
async fn spawn_batch(
    inner: &Arc<RuntimeInner>,
    ctx: &ToolCtx,
    tasks: Vec<SpawnRequest>,
    options: &SpawnOptions,
    batch: bool,
) -> Result<ToolOutcome> {
    let cfg = ctx.settings.subagent_config().await;
    let child_depth = ctx.depth.saturating_add(1);
    if ctx.depth > 0 && !cfg.allow_recursive {
        return Ok(ToolOutcome::err(
            "recursive subagents are disabled (settings.subagents.allow_recursive = false)",
        ));
    }
    if child_depth > cfg.max_depth {
        return Ok(ToolOutcome::err(format!(
            "subagent depth limit reached (settings.subagents.max_depth = {})",
            cfg.max_depth
        )));
    }
    if options.background && !cfg.allow_background {
        return Ok(ToolOutcome::err(
            "background subagents are disabled (settings.subagents.allow_background = false); \
             spawn without `background` and the call waits for the result",
        ));
    }

    let store = inner.deps.store.clone();
    let session_id = ctx.approvals.session_id.clone();

    // Capacity is checked for the whole batch before anything is registered: a
    // fan-out that started half its children and then refused the rest would
    // leave the parent holding ids it did not ask for and cannot easily attribute.
    // `max_background = 0` is "no ceiling", so the check is skipped rather than
    // compared against zero - which would refuse every background spawn.
    let live_background = inner.hub.background_count(&session_id.0);
    if options.background && cfg.max_background > 0 {
        let would_be = live_background.saturating_add(tasks.len());
        if would_be > cfg.max_background {
            return Ok(ToolOutcome::err(format!(
                "refused: {live_background} background agents are already live and this call starts \
                 {} more, over settings.subagents.max_background = {} (wait for one to finish, or \
                 raise the limit)",
                tasks.len(),
                cfg.max_background
            )));
        }
    }

    // The session's model pin decides the child's model; an explicit `model`
    // argument still overrides it for just that child. The same row carries the
    // session's worktree override, so one read answers both. A missing session is
    // not fatal: fall back to the default model and to the install-wide setting.
    let session_row = match store.get_session(&session_id.0).await {
        Ok(session) => session,
        Err(e) => {
            tracing::warn!(
                session = %session_id.0,
                error = %e,
                "could not read the session row; using the default model and install-wide subagent isolation"
            );
            None
        }
    };
    let session_selection = session_row
        .as_ref()
        .and_then(|session| session.model.clone());
    // Resolved once per batch: every child of this call shares one answer, which
    // is what makes "use worktrees for this round" mean the whole round.
    let worktree = resolve_worktrees(
        options.worktree,
        session_row.as_ref().and_then(|session| session.worktrees),
        cfg.worktrees,
    );
    let limits = inner.deps.settings.limits().await;
    let policy = inner.deps.settings.approval_policy().await;
    // A spawn's `model` is free text the caller typed; nothing downstream checks
    // it, and an id the endpoint cannot resolve kills the child on its *first*
    // provider call - after a slot, a claim and a transcript were spent. Resolve
    // it like a session selection, once, before any of that: the valid ids are
    // the models this install made available (any profile's catalog), and a
    // cross-profile id routes the child onto that profile.
    let base_model = inner
        .deps
        .settings
        .resolve_model(session_selection.as_ref())
        .await;

    let mut started: Vec<Arc<LiveAgent>> = Vec::with_capacity(tasks.len());
    let mut warnings: Vec<String> = Vec::new();
    let mut refusals: Vec<String> = Vec::new();

    for (index, spawn) in tasks.iter().enumerate() {
        // Resolve the requested model before the slot, the claim and the
        // transcript: the caller can fix a refusal in the same turn, and the
        // tool result is the only place it will read why.
        let mut model = base_model.clone();
        if let Some(requested) = spawn.model.as_deref() {
            match inner
                .deps
                .settings
                .resolve_requested_model(session_selection.as_ref(), requested)
                .await
            {
                ModelOverrideJudgement::Resolved(resolved) => model = resolved,
                ModelOverrideJudgement::Unjudged { .. } => {
                    // No catalog to judge against: keep the historical
                    // pass-through on the session's resolved endpoint.
                    model.model = requested.trim().to_string();
                }
                ModelOverrideJudgement::Unknown { requested, known } => {
                    refusals.push(format!(
                        "{}: {}",
                        spawn.label(index),
                        unknown_model_message(&known, &requested)
                    ));
                    continue;
                }
                ModelOverrideJudgement::Error(reason) => {
                    refusals.push(format!("{}: {reason}", spawn.label(index)));
                    continue;
                }
            }
        }

        // Background and nested blocking spawns take a slot only when one is
        // immediately available. In particular, a nested blocking waiter holds
        // an ancestor's permit until its own child returns, so waiting for a full
        // limiter is a wait-for cycle: the only task that could release a slot is
        // waiting for this spawn. Only a top-level blocking spawn may queue.
        let permit = if options.background || ctx.depth > 0 {
            match inner.subagents.try_acquire(cfg.max_concurrent) {
                Some(permit) => permit,
                None => {
                    let nested = if options.background || ctx.depth == 0 {
                        String::new()
                    } else {
                        "; nested blocking spawns never wait for a slot held by their own ancestry"
                            .to_string()
                    };
                    refusals.push(format!(
                        "{}: settings.subagents.max_concurrent = {} subagents are already running{}",
                        spawn.label(index),
                        cfg.max_concurrent,
                        nested
                    ));
                    continue;
                }
            }
        } else {
            match inner
                .subagents
                .acquire(cfg.max_concurrent, &ctx.cancel)
                .await
            {
                Some(permit) => permit,
                None => {
                    return Ok(ToolOutcome::err(
                        "cancelled while waiting for a free subagent slot",
                    ))
                }
            }
        };

        match prepare_child(
            inner,
            ctx,
            spawn,
            model,
            &limits,
            &policy,
            child_depth,
            &cfg,
            options.background,
            worktree,
        )
        .await
        {
            Ok(prepared) => {
                warnings.extend(prepared.warnings.clone());
                let live = start_child(
                    inner,
                    prepared,
                    permit,
                    ctx.cancel.child_token(),
                    cfg.clone(),
                )
                .await;
                started.push(live);
            }
            Err(e) => refusals.push(format!("{}: {e}", spawn.label(index))),
        }
    }

    if started.is_empty() {
        return Ok(ToolOutcome {
            ok: false,
            content: format!(
                "no subagent could be started:\n{}",
                refusals
                    .iter()
                    .map(|r| format!("- {r}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            truncated: false,
            meta: Some(serde_json::json!({ "agents": [], "refused": refusals })),
            images: Vec::new(),
        });
    }

    // A blocking call - and a `wait: true` background batch - collects the whole
    // set and answers with their summaries. Otherwise the parent gets the ids and
    // its turn back.
    let collect = !options.background || options.wait;
    if !collect {
        let ids: Vec<String> = started.iter().map(|a| a.id.0.clone()).collect();
        let mut content = format!(
            "started {} background subagent{}:\n",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" }
        );
        for agent in &started {
            content.push_str(&format!(
                "- `{}` {}{}\n",
                agent.id.0,
                agent.name,
                if agent.task.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", first_line(&agent.task))
                }
            ));
        }
        content.push_str(
            "\nThey keep working while you do. Collect them with \
             `wait_for_agents`, check on them with `list_subagents`, talk to them with \
             `send_message`, or stop them with `kill_agents`.",
        );
        if !warnings.is_empty() {
            content.push('\n');
            for warning in &warnings {
                content.push_str(&format!("\n- warning: {warning}"));
            }
        }
        let (content, cut) =
            truncate_output_with_advice(&content, MAX_SUMMARY_BYTES, SUMMARY_CUT_ADVICE);
        return Ok(ToolOutcome {
            ok: true,
            content,
            truncated: cut,
            meta: Some(serde_json::json!({
                "agents": ids,
                "background": true,
                "refused": refusals,
                "warnings": warnings,
            })),
            images: Vec::new(),
        });
    }

    // Wait for the whole set. A parent that is cancelled stops waiting, and the
    // children are cancelled with it by their own token.
    let mut results: Vec<(Arc<LiveAgent>, Option<AgentStatus>)> = Vec::with_capacity(started.len());
    for agent in &started {
        let timeout = Duration::from_secs(cfg.timeout_secs.max(1) + 1);
        let status = crate::hub::wait_for_settled(agent, timeout, &ctx.cancel).await;
        results.push((Arc::clone(agent), status.ok()));
    }

    let mut content = String::new();
    let mut ok = true;
    let mut meta_agents = Vec::new();
    for (agent, status) in &results {
        let record = store.get_agent(&agent.id.0).await.ok().flatten();
        let status_text = status
            .map(|s| s.as_str().to_string())
            .or_else(|| record.as_ref().map(|r| r.agent.status.clone()))
            .unwrap_or_else(|| "unknown".to_string());
        let summary = record
            .as_ref()
            .map(|r| r.agent.summary.clone())
            .unwrap_or_default();
        let error = record.as_ref().and_then(|r| r.agent.error.clone());
        let iterations = record.as_ref().map(|r| r.agent.iterations).unwrap_or(0);
        if status_text != AgentStatus::Completed.as_str() {
            ok = false;
        }
        content.push_str(&format!(
            "subagent `{}` ({}, {}) — {status_text}\niterations: {iterations}\n",
            agent.id.0, agent.name, agent.role
        ));
        if let Some(error) = &error {
            content.push_str(&format!("error: {error}\n"));
        }
        content.push('\n');
        content.push_str(if summary.trim().is_empty() {
            "(no summary produced)"
        } else {
            &summary
        });
        content.push_str("\n\n");
        meta_agents.push(serde_json::json!({
            "agent_id": agent.id.0,
            "name": agent.name,
            "status": status_text,
            "iterations": iterations,
            "error": error,
        }));
    }
    if !warnings.is_empty() {
        content.push_str("warnings:\n");
        for warning in &warnings {
            content.push_str(&format!("- {warning}\n"));
        }
    }
    if !refusals.is_empty() {
        content.push_str("refused:\n");
        for refusal in &refusals {
            content.push_str(&format!("- {refusal}\n"));
        }
    }
    let (content, cut) =
        truncate_output_with_advice(&content, MAX_SUMMARY_BYTES, SUMMARY_CUT_ADVICE);
    let _ = batch;
    Ok(ToolOutcome {
        ok: ok && refusals.is_empty(),
        content,
        truncated: cut,
        meta: Some(serde_json::json!({
            "agents": meta_agents,
            "refused": refusals,
            "warnings": warnings,
            "background": false,
        })),
        images: Vec::new(),
    })
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(120).collect::<String>())
    } else {
        line.to_string()
    }
}

/// The refusal message for a spawn's `model` argument that no profile of this
/// install offers.
///
/// This is the one id in the whole runtime that nothing derived: the caller
/// typed it into `spawn_agent`. `spawn_batch` resolves it through
/// [`Settings::resolve_requested_model`](kc_core::settings::Settings::resolve_requested_model),
/// which routes a valid id onto the profile that declares it and returns this
/// case only when the id is unknown *and* the install declared a catalog. An
/// install that declares no catalog is unjudged and keeps the pass-through,
/// so this message is never produced there.
///
/// Refusing returns the reason as tool output the caller can act on in the same
/// turn, which is strictly better than a dead subagent it has to guess about.
fn unknown_model_message(known: &[String], requested: &str) -> String {
    let requested = requested.trim();
    // Enough to choose from without turning a refusal into a wall of ids: the
    // caller only needs the shape of a real id, not the whole catalog.
    const SHOWN: usize = 8;
    let shown = known
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let more = known.len().saturating_sub(SHOWN);
    let tail = if more > 0 {
        format!("{shown} (and {more} more)")
    } else {
        shown
    };
    format!(
        "unknown model `{requested}`: this install offers {} model(s) and none has that id. \
         Omit `model` to inherit the session's model, or pass one of: {tail}",
        known.len()
    )
}

/// Owns a persisted child until its real loop is spawned.
///
/// Preparing the task message can await the store while the parent is cancelled.
/// The caller may then drop the whole spawn future, so cleanup after that await is
/// not sufficient. Dropping this guard transfers cancellation finalization and
/// claim release to a small tokio cleanup task; normal startup disarms it only
/// after the supervised child task exists.
struct UnsupervisedChildGuard {
    inner: Arc<RuntimeInner>,
    record: Option<AgentRecord>,
}

impl UnsupervisedChildGuard {
    fn new(inner: &Arc<RuntimeInner>, record: &AgentRecord) -> Self {
        Self {
            inner: Arc::clone(inner),
            record: Some(record.clone()),
        }
    }

    fn disarm(mut self) {
        self.record = None;
    }
}

impl Drop for UnsupervisedChildGuard {
    fn drop(&mut self) {
        let Some(mut record) = self.record.take() else {
            return;
        };
        record.status = AgentStatus::Cancelled.as_str().to_string();
        record.error = Some("cancelled before the subagent supervisor started".to_string());
        record.updated_at = now_ms();
        record.finished_at = Some(record.updated_at);
        let inner = Arc::clone(&self.inner);
        let agent_id = record.id.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(agent = %agent_id, "no tokio runtime available to finalize an unsupervised subagent");
            return;
        };
        runtime.spawn(async move {
            let store = inner.deps.store.clone();
            let summary = record.summary.clone();
            let error = record.error.clone();
            if let Err(e) = store.upsert_agent(&record).await {
                tracing::warn!(agent = %agent_id, error = %e, "could not finalize an unsupervised subagent");
            }
            if let Err(e) = store.release_claims(&agent_id).await {
                tracing::warn!(agent = %agent_id, error = %e, "could not release an unsupervised subagent's claims");
            }
            crate::events::EmitCtx::new(
                Arc::clone(&inner),
                record.session_id.clone(),
                record.parent_run_id.clone(),
                Some(agent_id.clone()),
            )
            .emit(Event::AgentFinished {
                agent_id,
                status: AgentStatus::Cancelled.as_str().to_string(),
                summary,
                error,
            });
        });
    }
}

/// Everything a child loop needs that does not depend on it being started yet.
struct PreparedChild {
    /// Present until `start_child` has spawned the loop that will finalize it.
    supervisor: UnsupervisedChildGuard,
    record: AgentRecord,
    child_depth: u8,
    model: kc_core::config::ModelConfig,
    limits: kc_core::config::Limits,
    approval_mode: String,
    /// The conversation the child starts from (its task, already persisted).
    history: Vec<Message>,
    journal: Arc<crate::rewind::StoreJournal>,
    /// The parent's workspace: a child works in the project it was spawned from,
    /// never anywhere else.
    workspace: Arc<kc_core::paths::Workspace>,
    /// Where this child runs, already resolved from the call argument, the
    /// session override and `subagents.worktrees` - and which of the three
    /// decided, so a refusal can name it.
    worktree: WorktreePlan,
    /// Path-claim warnings, reported with the result.
    warnings: Vec<String>,
}

/// Register an agent and everything around it, without starting its loop.
///
/// Kept apart from [`start_child`] so the blocking and background paths cannot
/// disagree about what a subagent *is*: same claims, same transcript, same depth,
/// same budget. The only difference between them is who awaits the loop.
#[allow(clippy::too_many_arguments)]
async fn prepare_child(
    inner: &Arc<RuntimeInner>,
    ctx: &ToolCtx,
    spawn: &SpawnRequest,
    model: kc_core::config::ModelConfig,
    limits: &kc_core::config::Limits,
    policy: &kc_core::config::ApprovalPolicy,
    child_depth: u8,
    cfg: &kc_core::config::SubagentConfig,
    background: bool,
    worktree: WorktreePlan,
) -> Result<PreparedChild> {
    let agent_id = AgentId::new();
    let short_id: String = {
        let tail: String = agent_id.0.chars().rev().take(8).collect();
        tail.chars().rev().collect()
    };
    let name = spawn
        .name
        .clone()
        .unwrap_or_else(|| format!("{}-{short_id}", spawn.role));
    let store = inner.deps.store.clone();
    let session_id = ctx.approvals.session_id.clone();
    let parent_run_id = ctx.approvals.run_id.clone();

    let mut record = AgentRecord {
        id: agent_id.0.clone(),
        session_id: session_id.0.clone(),
        parent_run_id: parent_run_id.0.clone(),
        parent_agent_id: ctx.approvals.agent_id.as_ref().map(|a| a.0.clone()),
        name: name.clone(),
        role: spawn.role.clone(),
        task: spawn.task.clone(),
        status: AgentStatus::Running.as_str().to_string(),
        model: model.model.clone(),
        iterations: 0,
        summary: String::new(),
        error: None,
        owned_paths: spawn.owned_paths.clone(),
        created_at: now_ms(),
        updated_at: now_ms(),
        finished_at: None,
        background,
        note: String::new(),
        progress: None,
    };
    store.upsert_agent(&record).await?;
    let supervisor = UnsupervisedChildGuard::new(inner, &record);
    ctx.emit(Event::AgentSpawned {
        agent_id: agent_id.0.clone(),
        name: name.clone(),
        role: spawn.role.clone(),
        task: spawn.task.clone(),
        model: model.model.clone(),
        parent_run_id: parent_run_id.0.clone(),
        background,
    });

    // --- claims ---------------------------------------------------------------
    let mut warnings: Vec<String> = Vec::new();
    if !spawn.owned_paths.is_empty() {
        // `claim_paths` records the claims and returns the *overlapping* claims
        // held by other agents (already normalised and ancestor-aware), so use
        // that result directly instead of re-deriving conflicts here.
        match store
            .claim_paths(&session_id.0, &agent_id.0, &spawn.owned_paths, "write")
            .await
        {
            Ok(conflicts) => {
                for claim in conflicts {
                    warnings.push(format!(
                        "path `{}` is also claimed by agent {}",
                        claim.path, claim.agent_id
                    ));
                }
            }
            Err(e) => warnings.push(format!("could not record path claims: {e}")),
        }
    }

    // --- child conversation ---------------------------------------------------
    let mut task_message = NewMessage::from_message(
        &session_id.0,
        Some(&parent_run_id.0),
        &Message::user(spawn.task.clone()),
    );
    task_message.agent_id = Some(agent_id.0.clone());
    if let Err(e) = store.append_message(&task_message).await {
        // Claims were recorded before the task message; release them on this
        // early return instead of leaking them for the rest of the session.
        if let Err(release) = store.release_claims(&agent_id.0).await {
            tracing::warn!(agent = %agent_id.0, error = %release, "could not release path claims");
        }
        record.status = AgentStatus::Failed.as_str().to_string();
        record.error = Some(format!("could not persist the subagent task: {e}"));
        record.updated_at = now_ms();
        record.finished_at = Some(now_ms());
        if store.upsert_agent(&record).await.is_ok() {
            ctx.emit(Event::AgentFinished {
                agent_id: agent_id.0.clone(),
                status: AgentStatus::Failed.as_str().to_string(),
                summary: String::new(),
                error: record.error.clone(),
            });
            supervisor.disarm();
        }
        return Err(e);
    }

    let journal = crate::rewind::StoreJournal::new(
        inner.deps.store.clone(),
        session_id.clone(),
        parent_run_id.clone(),
        Some(agent_id.clone()),
        Arc::clone(&ctx.workspace),
    );

    let _ = cfg;
    Ok(PreparedChild {
        supervisor,
        record,
        child_depth,
        model,
        limits: limits.clone(),
        approval_mode: policy.mode.as_str().to_string(),
        history: vec![Message::user(spawn.task.clone())],
        journal,
        workspace: Arc::clone(&ctx.workspace),
        worktree,
        warnings,
    })
}

/// Start a prepared child's loop in the background and return its live entry.
///
/// The permit travels into the task so the concurrency slot is held for exactly
/// as long as the loop runs, however it ends. `cfg` is read live by the caller:
/// the limiter's own copy is the construction-time default and would ignore a
/// saved settings change.
async fn start_child(
    inner: &Arc<RuntimeInner>,
    prepared: PreparedChild,
    permit: SubagentPermit,
    cancel: CancellationToken,
    cfg: kc_core::config::SubagentConfig,
) -> Arc<LiveAgent> {
    let PreparedChild {
        supervisor,
        record,
        child_depth,
        model,
        limits,
        approval_mode,
        history,
        journal,
        workspace,
        worktree,
        warnings,
    } = prepared;
    let agent_id = AgentId(record.id.clone());
    let session_id = crate::SessionId(record.session_id.clone());
    let parent_run_id = crate::RunId(record.parent_run_id.clone());

    let child_state = Arc::new(crate::RunState::new(
        parent_run_id.clone(),
        session_id.clone(),
        Some(agent_id.clone()),
        cancel.clone(),
        false,
    ));

    let live = inner.hub.insert(
        &record,
        parent_run_id.0.clone(),
        child_depth,
        cancel.clone(),
    );
    inner
        .runs
        .lock()
        .expect("runs lock")
        .insert(child_state.key(), Arc::clone(&child_state));

    let gate = RuntimeApprovalGate::new(
        Arc::clone(inner),
        session_id.clone(),
        parent_run_id.clone(),
        Some(agent_id.clone()),
        cancel.clone(),
        child_depth,
    );
    let approval_scope = gate.scope();
    let gate: Arc<dyn kc_core::tool::ApprovalGate> = Arc::new(gate);

    let spec = LoopSpec {
        session_id: session_id.clone(),
        run_id: parent_run_id.clone(),
        agent_id: Some(agent_id.clone()),
        depth: child_depth,
        is_subagent: true,
        budget: kc_core::prompt::Budget::for_subagent(&limits, &cfg),
        deadline: Instant::now() + Duration::from_secs(cfg.timeout_secs.max(1)),
        // The child's own window. Its request also carries a tool catalogue (a
        // child registry, so not necessarily the parent's), and that is taken off
        // this budget in the one place every request is built - `run_loop_inner` -
        // rather than here: reserving it in both places would charge the child
        // twice for the same JSON and leave it a third less room to work in.
        context_max_chars: limits.context_chars_for(&model),
        model,
        limits,
        approval_mode,
        gate,
        approval_scope,
        tools: Arc::new(child_tool_registry(inner, child_depth, &cfg)),
        cancel: cancel.clone(),
        // Inherited from the parent's tool context: a subagent works in the
        // same project as the run that spawned it.
        workspace,
        // A subagent changes the same files as its parent, so its writes go into
        // the same journal - with its own `agent_id`, which is what lets a rewind
        // name the delegated turn a change came from.
        journal: Some(journal),
        history,
        run_state: Some(Arc::clone(&child_state)),
        hub_entry: Some(Arc::clone(&live)),
    };

    let spawned_inner = Arc::clone(inner);
    let spawned_live = Arc::clone(&live);
    let spawned_record = record;
    let warnings_for_task = warnings;
    tokio::spawn(async move {
        run_child_loop(
            &spawned_inner,
            spawned_live,
            spawned_record,
            spec,
            _permit_holder(permit),
            warnings_for_task,
            worktree,
        )
        .await;
    });
    // From this point the loop owns terminal status and claim cleanup. Before
    // this statement, dropping `prepared` would have finalized it instead.
    supervisor.disarm();
    live
}

/// Keep a permit alive for the length of the task without naming it in a
/// signature. A `let _ = permit` inside the async block would be dropped at the
/// first suspension point by an optimising compiler that sees it unused, so the
/// permit is moved into an opaque holder instead.
fn _permit_holder(permit: SubagentPermit) -> SubagentPermit {
    permit
}

/// Drive one child's loop, then persist and report how it ended.
async fn run_child_loop(
    inner: &Arc<RuntimeInner>,
    live: Arc<LiveAgent>,
    record: AgentRecord,
    mut spec: LoopSpec,
    _permit: SubagentPermit,
    mut warnings: Vec<String>,
    plan: WorktreePlan,
) {
    let agent_id = live.id.clone();
    let session_id = live.session_id.clone();
    let cfg = inner.deps.settings.subagent_config().await;
    let timeout_secs = cfg.timeout_secs.max(1);

    // Where the child works. The default is the parent's live workspace: a child
    // sees dirty and untracked parent state, its edits land immediately, and the
    // journal and its tools resolve the same tree, so a rewind never targets a
    // temporary checkout. `worktree` is the spawn's already-resolved answer - its
    // own `worktree` argument, else this session's override, else
    // `settings.subagents.worktrees` - so the decision and the refusal that can
    // follow are made where the caller can still see which of the three asked.
    let (spec, worktree) = if plan.on {
        let root = spec.workspace.root();
        let guard = match crate::worktree::WorktreeGuard::isolated(root, &agent_id.0) {
            Ok(guard) => guard,
            Err(e) => {
                let reason = match plan.source.refusal_note() {
                    Some(note) => format!("{e} ({note})"),
                    None => e.to_string(),
                };
                return finish_failed_spawn(inner, &live, record, warnings, reason).await;
            }
        };
        warnings.extend(guard.warnings().iter().cloned());
        // A checkout inside the parent workspace, keeping the parent's extra
        // roots: the child is confined to its own tree without losing access to
        // anything the project deliberately allowed.
        let roots = spec.workspace.extra_roots().to_vec();
        match kc_core::paths::Workspace::with_extra_roots(guard.path(), roots) {
            Ok(ws) => {
                let ws = Arc::new(ws);
                spec.workspace = Arc::clone(&ws);
                // The journal resolves a recorded path against its workspace, both
                // when it captures the before-image and when a rewind restores it.
                // Reusing the parent's journal would capture from the child's
                // checkout and then restore into the parent tree, so the child gets
                // its own: its undo is confined to its own checkout.
                spec.journal = Some(crate::rewind::StoreJournal::new(
                    inner.deps.store.clone(),
                    crate::SessionId(session_id.clone()),
                    crate::RunId(live.run_id.clone()),
                    Some(agent_id.clone()),
                    ws,
                ));
                (spec, guard)
            }
            Err(e) => {
                let (_, cleanup) = guard.finish();
                warnings.extend(cleanup);
                return finish_failed_spawn(inner, &live, record, warnings, e.to_string()).await;
            }
        }
    } else {
        let guard = crate::worktree::WorktreeGuard::shared(spec.workspace.root());
        (spec, guard)
    };

    let guarded = std::panic::AssertUnwindSafe(crate::agent::run_loop(Arc::clone(inner), spec))
        .catch_unwind();
    tokio::pin!(guarded);
    let hard_deadline = tokio::time::sleep(Duration::from_secs(timeout_secs));
    tokio::pin!(hard_deadline);

    enum Exit {
        Completed(LoopOutcome),
        TimedOut,
        ParentCancelled,
    }

    let exit = tokio::select! {
        biased;
        _ = live.cancel.cancelled() => Exit::ParentCancelled,
        _ = &mut hard_deadline => Exit::TimedOut,
        result = guarded.as_mut() => Exit::Completed(match result {
            Ok(outcome) => outcome,
            Err(payload) => LoopOutcome {
                status: RunStatus::Failed,
                error: Some(format!(
                    "subagent panicked: {}",
                    crate::agent::panic_message(payload.as_ref())
                )),
                text: String::new(),
                iterations: 0,
            },
        }),
    };

    let timed_out = matches!(exit, Exit::TimedOut);
    let killed = *live.killed.lock().expect("killed lock");
    let outcome = match exit {
        Exit::Completed(outcome) => outcome,
        Exit::TimedOut | Exit::ParentCancelled => {
            // Fire the child token, then keep polling the loop for a short grace
            // period: in-flight tools select on the token, kill their process
            // groups and return. Only then is the future dropped.
            live.cancel.cancel();
            let _ = tokio::time::timeout(SUBAGENT_CANCEL_GRACE, guarded.as_mut()).await;

            let iterations = inner
                .runs
                .lock()
                .expect("runs lock")
                .get(&agent_id.0)
                .map(|state| state.iterations.load(Ordering::Relaxed))
                .unwrap_or(0);
            if timed_out {
                LoopOutcome {
                    status: RunStatus::Failed,
                    error: Some(format!(
                        "subagent timed out after {timeout_secs}s (settings.subagents.timeout_secs)"
                    )),
                    text: String::new(),
                    iterations,
                }
            } else {
                LoopOutcome {
                    status: RunStatus::Cancelled,
                    error: None,
                    text: String::new(),
                    iterations,
                }
            }
        }
    };
    inner.runs.lock().expect("runs lock").remove(&agent_id.0);

    // --- finish ---------------------------------------------------------------
    // A shared guard owns no checkout and removes nothing: every edit the child
    // completed remains in the parent workspace. An isolated guard commits what
    // the child changed to its own branch and removes the checkout, and the
    // handoff is appended to the summary so the parent can find the work.
    let (handoff, worktree_warnings) = worktree.finish();
    // These are not ordinary warnings: they are the only signal that a child's
    // changes exist but could not be handed back. They go into the summary, which
    // is what the parent reads, and not just into the log.
    let worktree_trouble = worktree_warnings.join("; ");
    warnings.extend(worktree_warnings);

    let (summary_raw, _) =
        truncate_output_with_advice(&outcome.text, MAX_SUMMARY_BYTES, SUMMARY_CUT_ADVICE);
    let mut summary = if summary_raw.trim().is_empty() {
        outcome.error.clone().unwrap_or_default()
    } else {
        summary_raw
    };
    if let Some(handoff) = &handoff {
        match &handoff.branch {
            Some(branch) => summary.push_str(&format!(
                "\n\n[worktree] {files} file(s) changed on branch `{branch}`, cut from {base}. \
                 Nothing was merged into the parent's working tree: review with \
                 `git diff {base}..{branch}`, then merge or delete the branch.",
                files = handoff.files,
                base = handoff.base,
            )),
            None => summary.push_str(&format!(
                "\n\n[worktree] {files} file(s) were changed but could NOT be committed to a \
                 branch; the checkout is still on disk at {path} and holds the only copy. \
                 Do not delete it before copying the work out.",
                files = handoff.files,
                path = handoff.checkout.display(),
            )),
        }
    }
    if !worktree_trouble.is_empty() {
        summary.push_str(&format!("\n\n[worktree] {worktree_trouble}"));
    }
    let agent_status = match outcome.status {
        RunStatus::Completed => AgentStatus::Completed,
        // A kill is a cancellation the parent asked for, so say so plainly rather
        // than reporting a generic stop.
        RunStatus::Cancelled => AgentStatus::Cancelled,
        RunStatus::Failed => AgentStatus::Failed,
        RunStatus::LimitReached => AgentStatus::Failed,
        RunStatus::Interrupted => AgentStatus::Interrupted,
        RunStatus::Running => AgentStatus::Running,
    };
    let error = match outcome.status {
        RunStatus::LimitReached => Some(
            outcome
                .error
                .clone()
                .unwrap_or_else(|| "subagent stopped at its iteration or time limit".to_string()),
        ),
        _ => outcome.error.clone(),
    };

    let store = inner.deps.store.clone();
    let mut final_record = record;
    final_record.status = agent_status.as_str().to_string();
    final_record.iterations = outcome.iterations;
    final_record.summary = summary.clone();
    final_record.error = error.clone();
    final_record.note = live.note.lock().expect("note lock").clone();
    final_record.progress = *live.progress.lock().expect("progress lock");
    final_record.updated_at = now_ms();
    final_record.finished_at = Some(now_ms());
    if let Err(e) = store.upsert_agent(&final_record).await {
        warnings.push(format!("could not update the agent record: {e}"));
    }
    if let Err(e) = store.release_claims(&agent_id.0).await {
        warnings.push(format!("could not release path claims: {e}"));
    }
    for warning in &warnings {
        tracing::warn!(agent = %agent_id.0, warning, "subagent finished with a warning");
    }

    let ctx = crate::events::EmitCtx::new(
        Arc::clone(inner),
        session_id.clone(),
        live.run_id.clone(),
        Some(agent_id.0.clone()),
    );
    ctx.emit(Event::AgentFinished {
        agent_id: agent_id.0.clone(),
        status: agent_status.as_str().to_string(),
        summary: summary.clone(),
        error: error.clone(),
    });

    // Mark the live entry settled before removing it: a `wait_for_agents` that is
    // parked on it right now must observe the real status, not "unknown agent".
    crate::hub::AgentHub::mark_settled(&live, agent_status);
    inner.hub.finish(&agent_id.0, agent_status);

    // Only a failure is worth interrupting a human for: a subagent that succeeded
    // is reported by its parent run's own completion. A *background* agent is the
    // exception - nobody is waiting on the tool result that used to carry the
    // failure, so the failure itself has to be the signal.
    if agent_status == AgentStatus::Failed || agent_status == AgentStatus::Interrupted {
        crate::notify::agent_failed(
            inner,
            &session_id,
            &ctx.sink(),
            &agent_id.0,
            &final_record.name,
            error.as_deref(),
        )
        .await;
    }
    let _ = killed;
}

/// End a spawn that could not start, through the same paths a failed child uses:
/// the stored row, the `agent_finished` event, the hub and the failure notice.
///
/// Used when the child's workspace cannot be prepared (an isolated checkout in a
/// directory that is not a git repository, for instance). Without this the running
/// row inserted before the loop starts would be left behind with no owner.
async fn finish_failed_spawn(
    inner: &Arc<RuntimeInner>,
    live: &Arc<LiveAgent>,
    record: AgentRecord,
    warnings: Vec<String>,
    reason: String,
) {
    let agent_id = live.id.clone();
    let session_id = live.session_id.clone();
    inner.runs.lock().expect("runs lock").remove(&agent_id.0);

    let mut final_record = record;
    final_record.status = AgentStatus::Failed.as_str().to_string();
    final_record.summary = reason.clone();
    final_record.error = Some(reason.clone());
    final_record.note = live.note.lock().expect("note lock").clone();
    final_record.progress = *live.progress.lock().expect("progress lock");
    final_record.updated_at = now_ms();
    final_record.finished_at = Some(now_ms());
    let store = inner.deps.store.clone();
    if let Err(e) = store.upsert_agent(&final_record).await {
        tracing::warn!(agent = %agent_id.0, "could not update the failed agent record: {e}");
    }
    if let Err(e) = store.release_claims(&agent_id.0).await {
        tracing::warn!(agent = %agent_id.0, "could not release path claims: {e}");
    }
    for warning in &warnings {
        tracing::warn!(agent = %agent_id.0, warning, "subagent could not start");
    }

    let ctx = crate::events::EmitCtx::new(
        Arc::clone(inner),
        session_id.clone(),
        live.run_id.clone(),
        Some(agent_id.0.clone()),
    );
    ctx.emit(Event::AgentFinished {
        agent_id: agent_id.0.clone(),
        status: AgentStatus::Failed.as_str().to_string(),
        summary: reason.clone(),
        error: Some(reason.clone()),
    });
    crate::hub::AgentHub::mark_settled(live, AgentStatus::Failed);
    inner.hub.finish(&agent_id.0, AgentStatus::Failed);
    crate::notify::agent_failed(
        inner,
        &session_id,
        &ctx.sink(),
        &agent_id.0,
        &final_record.name,
        Some(&reason),
    )
    .await;
}

/// Parent tool set minus the delegate tools, unless recursion is allowed at this
/// depth (in which case they stay available).
fn child_tool_registry(
    inner: &Arc<RuntimeInner>,
    depth: u8,
    cfg: &kc_core::config::SubagentConfig,
) -> ToolRegistry {
    let can_delegate = cfg.allow_recursive && depth < cfg.max_depth;
    let parent = inner.tool_view();
    let mut registry = ToolRegistry::new();
    for name in parent.names() {
        // `subagent_worktrees` is the top-level agent's alone, at every depth and
        // with recursion anywhere on the scale: it writes this session's override
        // and, globally, the install default - neither of which a child was
        // delegated. Dropping it here (rather than refusing inside the tool) keeps
        // it out of the child's catalogue, so a model never plans around a tool it
        // cannot use.
        if name == SUBAGENT_WORKTREES_TOOL {
            continue;
        }
        if !can_delegate && DELEGATION_ONLY_TOOLS.contains(&name.as_str()) {
            continue;
        }
        if let Some(tool) = parent.get(&name) {
            registry.register(tool);
        }
    }
    // `send_message`, `report_status`, `list_subagents` and `agent_history` stay
    // available at every depth, even with recursion off: a child that can only
    // work cannot report that it is blocked, and a parent cannot steer a child it
    // cannot see. What recursion buys is the ability to spawn and to *control*
    // agents - the tools that can multiply work or stop somebody else's.
    for name in COORDINATION_TOOLS {
        if registry.get(name).is_none() {
            let mut fresh = ToolRegistry::new();
            install_delegate_tools(inner, &mut fresh);
            if let Some(tool) = fresh.get(name) {
                registry.register(tool);
            }
        }
    }
    if can_delegate {
        // The parent's view already carries these; if it somehow does not (an
        // embedder that installed its own registry), install ours so a recursive
        // child can still delegate.
        for name in DELEGATION_ONLY_TOOLS {
            if registry.get(name).is_none() {
                let mut fresh = ToolRegistry::new();
                install_delegate_tools(inner, &mut fresh);
                if let Some(tool) = fresh.get(name) {
                    registry.register(tool);
                }
            }
        }
    }
    registry
}

// ---------------------------------------------------------------------------
// control tools
// ---------------------------------------------------------------------------

/// Resolve which live agents a control call addresses.
///
/// `agents` names ids explicitly; an empty list means "everything I started",
/// which is the subtree below the caller (or every live agent in the session for
/// a top-level run). Ids the caller does not own are refused with
/// [`ControlError::NotOwned`] rather than silently skipped.
fn resolve_targets(
    inner: &Arc<RuntimeInner>,
    session_id: &str,
    caller: Option<&str>,
    requested: &[String],
) -> std::result::Result<Vec<Arc<LiveAgent>>, ControlError> {
    if requested.is_empty() {
        return Ok(inner.hub.descendants_of(session_id, caller));
    }
    let owned: std::collections::HashSet<String> = inner
        .hub
        .descendants_of(session_id, caller)
        .iter()
        .map(|agent| agent.id.0.clone())
        .collect();
    let mut out = Vec::with_capacity(requested.len());
    for id in requested {
        let live = inner
            .hub
            .get(id)
            .ok_or_else(|| ControlError::Unknown(id.clone()))?;
        if !owned.contains(id) {
            return Err(ControlError::NotOwned {
                agent: id.clone(),
                owner: live
                    .parent_agent_id
                    .clone()
                    .map(|parent| format!("agent `{parent}`"))
                    .unwrap_or_else(|| "the main agent".to_string()),
            });
        }
        out.push(live);
    }
    Ok(out)
}

fn parse_agent_ids(request: &ToolCallRequest, key: &str) -> Result<Vec<String>> {
    match request.arguments.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(value) => serde_json::from_value::<Vec<String>>(value.clone()).map_err(|_| {
            kc_core::Error::BadRequest(format!("`{key}` must be an array of strings"))
        }),
    }
}

fn caller_id(ctx: &ToolCtx) -> Option<String> {
    ctx.approvals.agent_id.as_ref().map(|id| id.0.clone())
}

fn caller_label(ctx: &ToolCtx) -> String {
    match &ctx.approvals.agent_id {
        Some(id) => format!("agent `{}`", id.0),
        None => "the main agent".to_string(),
    }
}

/// Whether a persisted row is in the caller's transitive ownership subtree.
///
/// Settled agents are no longer in the hub, so a named late wait has to walk the
/// same parent links in SQLite instead of accidentally becoming able to inspect
/// any historical row in the process. Cycles and missing ancestors fail closed.
async fn owns_persisted_agent(
    inner: &Arc<RuntimeInner>,
    session_id: &str,
    caller: Option<&str>,
    record: &AgentRecord,
) -> Result<bool> {
    if record.session_id != session_id {
        return Ok(false);
    }
    let Some(caller) = caller else {
        return Ok(true);
    };
    let mut ancestor = record.parent_agent_id.clone();
    let mut visited = std::collections::HashSet::new();
    visited.insert(record.id.clone());
    while let Some(id) = ancestor {
        if id == caller {
            return Ok(true);
        }
        if !visited.insert(id.clone()) {
            return Ok(false);
        }
        let Some(parent) = inner.deps.store.get_agent(&id).await? else {
            return Ok(false);
        };
        if parent.agent.session_id != session_id {
            return Ok(false);
        }
        ancestor = parent.agent.parent_agent_id;
    }
    Ok(false)
}

pub(crate) struct SendMessageTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for SendMessageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: SEND_MESSAGE_TOOL.to_string(),
            description: "Deliver a message to one or more other agents in this session - a subagent \
                          you spawned, a sibling, or your own parent - as their next turn. It does not interrupt \
                          them mid-turn: it is queued and they read it at the top of their next \
                          iteration. Omit `agents` to message your parent."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "What to say. Be specific: it becomes a user turn in their conversation."
                    },
                    "agents": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Agent ids to message. Omit (or leave empty) to message your parent."
                    }
                },
                "required": ["message"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let message = match request
            .opt_str("message")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(message) => message.to_string(),
            None => return ToolOutcome::err("`message` may not be empty"),
        };
        let targets = match parse_agent_ids(&request, "agents") {
            Ok(targets) => targets,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let from = caller_id(&ctx);
        let from_name = match &from {
            Some(id) => inner
                .hub
                .get(id)
                .map(|live| live.name.clone())
                .unwrap_or_else(|| id.clone()),
            None => "the main agent".to_string(),
        };
        let session_id = ctx.approvals.session_id.0.clone();
        // The root run id is part of the mailbox address. An unwinding child keeps
        // its old context even after a successor starts, and must still target the
        // retired generation rather than whichever run now owns the session.
        let run_id = ctx.approvals.run_id.0.clone();
        let envelope = |content: String| AgentMessage {
            from: from.clone(),
            from_name: from_name.clone(),
            content,
            sent_at: now_ms(),
        };

        let mut lines = Vec::new();
        let mut delivered = 0usize;

        if targets.is_empty() {
            // No explicit target means "my parent". For a top-level run that is
            // nobody, which is a mistake worth naming rather than silently
            // accepting.
            let parent = from.as_deref().and_then(|id| {
                inner
                    .hub
                    .get(id)
                    .and_then(|live| live.parent_agent_id.clone())
            });
            match parent {
                Some(parent) => match inner.hub.deliver_in_session(
                    &session_id,
                    &parent,
                    envelope(message.clone()),
                ) {
                    MailboxDelivery::Queued { depth } => {
                        delivered += 1;
                        ctx.emit(Event::AgentMessage {
                            from_agent_id: from.clone(),
                            to: parent.clone(),
                            state: "queued".to_string(),
                        });
                        lines.push(format!(
                            "queued for your parent `{parent}` ({depth} per iteration)"
                        ));
                    }
                    MailboxDelivery::Full { .. } => lines.push(format!(
                        "your parent `{parent}` already has {MAX_MAILBOX} unread messages; nothing was queued"
                    )),
                    MailboxDelivery::NotLive => lines.push(format!(
                        "could not deliver to `{parent}`: that agent is no longer running"
                    )),
                },
                None => {
                    // Not spawned by an agent, so the parent is the session's run.
                    match inner.hub.deliver_to_run(
                        &session_id,
                        &run_id,
                        envelope(message.clone()),
                    ) {
                        MailboxDelivery::Queued { .. } => {
                            delivered += 1;
                            ctx.emit(Event::AgentMessage {
                                from_agent_id: from.clone(),
                                to: format!("run:{run_id}"),
                                state: "queued".to_string(),
                            });
                            lines.push("queued for the main agent's run".to_string());
                        }
                        MailboxDelivery::Full { .. } => lines.push(format!(
                            "the parent's mailbox is full ({MAX_MAILBOX} messages); nothing was queued"
                        )),
                        MailboxDelivery::NotLive => lines.push(
                            "could not deliver to the main agent: that run is no longer running; nothing was queued"
                                .to_string(),
                        ),
                    }
                }
            }
        } else {
            for target in &targets {
                match inner
                    .hub
                    .deliver_in_session(&session_id, target, envelope(message.clone()))
                {
                    MailboxDelivery::Queued { .. } => {
                        delivered += 1;
                        ctx.emit(Event::AgentMessage {
                            from_agent_id: from.clone(),
                            to: target.clone(),
                            state: "queued".to_string(),
                        });
                        lines.push(format!("queued for `{target}`"));
                    }
                    MailboxDelivery::Full { .. } => lines.push(format!(
                        "`{target}` already has {MAX_MAILBOX} unread messages; nothing was queued"
                    )),
                    MailboxDelivery::NotLive => lines.push(format!(
                        "`{target}` is not a live agent in this session; nothing was queued"
                    )),
                }
            }
        }

        let content = format!(
            "delivered to {delivered} agent{}:\n{}",
            if delivered == 1 { "" } else { "s" },
            lines.join("\n")
        );
        ToolOutcome {
            ok: delivered > 0,
            content,
            truncated: false,
            meta: Some(serde_json::json!({ "delivered": delivered })),
            images: Vec::new(),
        }
    }
}

pub(crate) struct WaitForAgentsTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for WaitForAgentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: WAIT_FOR_AGENTS_TOOL.to_string(),
            description: "Wait until background subagents finish, then return their summaries. \
                          Omit `agents` to wait for every subagent you started, including ones \
                          they started. Bounded by `timeout_secs`: a wait that runs out reports \
                          which agents are still running instead of hanging."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agents": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Agent ids to wait for. Omit to wait for everything you started."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "How long to wait at most. Defaults to subagents.wait_timeout_secs."
                    }
                },
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let requested = match parse_agent_ids(&request, "agents") {
            Ok(ids) => ids,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let cfg = ctx.settings.subagent_config().await;
        let timeout = Duration::from_secs(
            request
                .opt_u64("timeout_secs")
                .unwrap_or(cfg.wait_timeout_secs)
                .clamp(1, cfg.timeout_secs.max(1) + 1),
        );

        let session_id = ctx.approvals.session_id.0.clone();
        let caller = caller_id(&ctx);
        let store = inner.deps.store.clone();
        let mut targets = Vec::new();
        let mut finished: Vec<(String, AgentStatus, Option<AgentRecord>)> = Vec::new();

        if requested.is_empty() {
            targets = match resolve_targets(&inner, &session_id, caller.as_deref(), &requested) {
                Ok(targets) => targets,
                Err(e) => return ToolOutcome::err(e.message()),
            };
        } else {
            // Resolve each named live target with the existing hub ownership rule.
            // If its entry has already been removed, an owned terminal SQLite row
            // is an equally valid result: the caller named it and needs its stored
            // summary, not an `Unknown` error caused by the timing of the cleanup.
            let owned: std::collections::HashSet<String> = inner
                .hub
                .descendants_of(&session_id, caller.as_deref())
                .iter()
                .map(|agent| agent.id.0.clone())
                .collect();
            for id in &requested {
                if let Some(live) = inner.hub.get(id) {
                    if live.session_id != session_id {
                        return ToolOutcome::err(ControlError::Unknown(id.clone()).message());
                    }
                    if !owned.contains(id) {
                        return ToolOutcome::err(
                            ControlError::NotOwned {
                                agent: id.clone(),
                                owner: live
                                    .parent_agent_id
                                    .clone()
                                    .map(|parent| format!("agent `{parent}`"))
                                    .unwrap_or_else(|| "the main agent".to_string()),
                            }
                            .message(),
                        );
                    }
                    targets.push(live);
                    continue;
                }

                let summary = match store.get_agent(id).await {
                    Ok(Some(summary)) => summary,
                    Ok(None) => {
                        return ToolOutcome::err(ControlError::Unknown(id.clone()).message())
                    }
                    Err(e) => return ToolOutcome::err(format!("could not read agent `{id}`: {e}")),
                };
                let record = summary.agent;
                if record.session_id != session_id {
                    return ToolOutcome::err(ControlError::Unknown(id.clone()).message());
                }
                let owned =
                    match owns_persisted_agent(&inner, &session_id, caller.as_deref(), &record)
                        .await
                    {
                        Ok(owned) => owned,
                        Err(e) => {
                            return ToolOutcome::err(format!(
                                "could not verify ownership of agent `{id}`: {e}"
                            ))
                        }
                    };
                if !owned {
                    return ToolOutcome::err(
                        ControlError::NotOwned {
                            agent: id.clone(),
                            owner: record
                                .parent_agent_id
                                .as_ref()
                                .map(|parent| format!("agent `{parent}`"))
                                .unwrap_or_else(|| "the main agent".to_string()),
                        }
                        .message(),
                    );
                }
                let Some(status) = AgentStatus::parse(&record.status) else {
                    return ToolOutcome::err(format!(
                        "agent `{id}` has unknown persisted status `{}`",
                        record.status
                    ));
                };
                if status.is_live() {
                    return ToolOutcome::err(format!(
                        "agent `{id}` has persisted status `{}` but no live supervisor",
                        status.as_str()
                    ));
                }
                finished.push((id.clone(), status, Some(record)));
            }
        }

        if targets.is_empty() && finished.is_empty() {
            return ToolOutcome::ok(format!(
                "{} has no live subagents to wait for",
                caller_label(&ctx)
            ));
        }

        let deadline = Instant::now() + timeout;
        let mut still_running: Vec<String> = Vec::new();
        for agent in &targets {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let status = crate::hub::wait_for_settled(agent, remaining, &ctx.cancel).await;
            match status {
                Ok(status) | Err(Some(status)) => {
                    let record = store.get_agent(&agent.id.0).await.ok().flatten();
                    finished.push((agent.id.0.clone(), status, record.map(|row| row.agent)));
                }
                Err(None) => still_running.push(agent.id.0.clone()),
            }
            if ctx.cancel.is_cancelled() {
                break;
            }
        }

        let mut content = String::new();
        if !finished.is_empty() {
            content.push_str(&format!(
                "{} agent{} finished:\n",
                finished.len(),
                if finished.len() == 1 { "" } else { "s" }
            ));
            for (id, status, record) in &finished {
                content.push_str(&format!("\n`{id}` — {}\n", status.as_str()));
                if let Some(record) = record {
                    if let Some(error) = &record.error {
                        content.push_str(&format!("error: {error}\n"));
                    }
                    content.push_str(if record.summary.trim().is_empty() {
                        "(no summary produced)\n"
                    } else {
                        &record.summary
                    });
                    content.push('\n');
                }
            }
        }
        if !still_running.is_empty() {
            content.push_str(&format!(
                "\nstill running after {}s: {}\n\nThey keep working; wait again, or message them.",
                timeout.as_secs(),
                still_running.join(", ")
            ));
        }
        let (content, cut) =
            truncate_output_with_advice(&content, MAX_SUMMARY_BYTES, SUMMARY_CUT_ADVICE);
        ToolOutcome {
            ok: still_running.is_empty(),
            content,
            truncated: cut,
            meta: Some(serde_json::json!({
                "finished": finished
                    .iter()
                    .map(|(id, status, _)| serde_json::json!({ "agent_id": id, "status": status.as_str() }))
                    .collect::<Vec<_>>(),
                "still_running": still_running,
            })),
            images: Vec::new(),
        }
    }
}

pub(crate) struct ListSubagentsTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for ListSubagentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: LIST_SUBAGENTS_TOOL.to_string(),
            description: "List the live subagents you can see - their ids, status, iterations, \
                          unread messages and last reported note - plus the ones that already \
                          finished. Use it to decide whether to wait, message, or stop something."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "all": {
                        "type": "boolean",
                        "description": "Include agents in this session that you did not start."
                    }
                },
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let all = request.opt_bool("all").unwrap_or(false);
        let session_id = ctx.approvals.session_id.0.clone();
        let caller = caller_id(&ctx);
        let live = if all {
            inner.hub.list(&session_id)
        } else {
            inner.hub.descendants_of(&session_id, caller.as_deref())
        };

        let mut content = String::new();
        let mut meta = Vec::new();
        if live.is_empty() {
            content.push_str("no live subagents");
        } else {
            content.push_str(&format!("{} live subagent(s):\n", live.len()));
            for agent in &live {
                let paused = agent.is_paused();
                let status = if paused { "paused" } else { "running" };
                let note = agent.note.lock().expect("note lock").clone();
                let progress = *agent.progress.lock().expect("progress lock");
                content.push_str(&format!(
                    "\n- `{}` {} ({}) — {status}, depth {}, {}, {} unread",
                    agent.id.0,
                    agent.name,
                    agent.role,
                    agent.depth,
                    if agent.model.is_empty() {
                        "default model".to_string()
                    } else {
                        agent.model.clone()
                    },
                    agent.queued_mail()
                ));
                if let Some(progress) = progress {
                    content.push_str(&format!(", {:.0}% according to itself", progress * 100.0));
                }
                content.push('\n');
                content.push_str(&format!("  task: {}\n", first_line(&agent.task)));
                if !note.trim().is_empty() {
                    content.push_str(&format!("  says: {note}\n"));
                }
                meta.push(serde_json::json!({
                    "agent_id": agent.id.0,
                    "name": agent.name,
                    "role": agent.role,
                    "status": status,
                    "model": agent.model,
                    "depth": agent.depth,
                    "background": agent.background,
                    "unread": agent.queued_mail(),
                    "note": note,
                    "progress": progress,
                    "parent_agent_id": agent.parent_agent_id,
                }));
            }
        }

        // Recently finished agents are half of what a caller wants to know
        // ("did the one I started an hour ago ever finish?"), and they are cheap:
        // the rows are already there.
        if let Ok(agents) = inner.deps.store.list_agents(&session_id).await {
            let mut finished: Vec<&AgentRecord> = agents
                .iter()
                .filter(|agent| !agent.is_live())
                .filter(|agent| all || caller.is_none() || agent.parent_agent_id == caller)
                .collect();
            finished.sort_by_key(|agent| std::cmp::Reverse(agent.updated_at));
            finished.truncate(10);
            if !finished.is_empty() {
                content.push_str("\n\nrecently finished:\n");
                for agent in finished {
                    content.push_str(&format!(
                        "- `{}` {} — {}{}\n",
                        agent.id,
                        agent.name,
                        agent.status,
                        if agent.error.is_some() {
                            " (with an error)"
                        } else {
                            ""
                        }
                    ));
                }
            }
        }

        let (content, cut) =
            truncate_output_with_advice(&content, MAX_SUMMARY_BYTES, SUMMARY_CUT_ADVICE);
        ToolOutcome {
            ok: true,
            content,
            truncated: cut,
            meta: Some(serde_json::json!({ "agents": meta })),
            images: Vec::new(),
        }
    }
}

pub(crate) struct AgentHistoryTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for AgentHistoryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: AGENT_HISTORY_TOOL.to_string(),
            description: "Read one agent's transcript: its task, what it did, the tools it called \
                          and the result it produced. This is the audit trail behind a summary."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string", "description": "The agent whose transcript to read." },
                    "before": { "type": "integer", "description": "Exclusive message id cursor for older messages." },
                    "messages": {
                        "type": "integer",
                        "description": "How many of the newest messages to read (default 60)."
                    }
                },
                "required": ["agent_id"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let Some(agent_id) = request
            .opt_str("agent_id")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return ToolOutcome::err("`agent_id` is required");
        };
        // The roster is per session, so reading is too: an id from another
        // session would otherwise be a way to read work this agent cannot see.
        match inner.deps.store.get_agent(agent_id).await {
            Ok(Some(agent)) if agent.agent.session_id == ctx.approvals.session_id.0 => {}
            Ok(Some(_)) => {
                return ToolOutcome::err(format!(
                    "agent `{agent_id}` belongs to another session; only this session's agents are visible"
                ))
            }
            Ok(None) => return ToolOutcome::err(format!("no agent `{agent_id}` exists")),
            Err(e) => return ToolOutcome::err(format!("could not look up `{agent_id}`: {e}")),
        }
        let limit = request.opt_u64("messages").unwrap_or(60).clamp(1, 500) as usize;
        let before = match request.arguments.get("before") {
            None => None,
            Some(value) => match value.as_i64().filter(|id| *id > 0) {
                Some(id) => Some(id),
                None => return ToolOutcome::err("`before` must be a positive message id"),
            },
        };
        let page = match inner
            .deps
            .store
            .list_message_page(
                &ctx.approvals.session_id.0,
                MessageScope::Agent(agent_id),
                before,
                limit,
                MAX_HISTORY_BYTES,
            )
            .await
        {
            Ok(page) => page,
            Err(e) => return ToolOutcome::err(format!("could not read the transcript: {e}")),
        };
        let messages = page.messages;
        if messages.is_empty() {
            return ToolOutcome::ok(format!(
                "agent `{agent_id}` has no stored transcript on this page"
            ));
        }
        // Put pagination information first: the runtime may truncate tool output
        // again to a smaller configured limit before the model sees it.
        let header = if page.has_more {
            format!(
                "Older messages available: call agent_history with agent_id={agent_id:?} and before={}.\n\n",
                messages[0].id
            )
        } else {
            "No older messages on this transcript page.\n\n".to_string()
        };
        let mut body = String::new();
        for message in &messages {
            let role = message.role.as_str();
            body.push_str(&format!("[{role}]\n{}\n\n", message.content));
            for call in &message.tool_calls {
                body.push_str(&format!("  -> {}({})\n", call.name, call.arguments));
            }
        }
        let (body, cut) = truncate_output(&body, MAX_HISTORY_BYTES.saturating_sub(header.len()));
        let content = format!("{header}{body}");
        ToolOutcome {
            ok: true,
            content,
            truncated: cut || page.has_more,
            meta: Some(serde_json::json!({
                "agent_id": agent_id,
                "messages": messages.len(),
                "messages_truncated": page.has_more,
                "messages_before": if page.has_more { messages.first().map(|m| m.id) } else { None },
            })),
            images: Vec::new(),
        }
    }
}

pub(crate) struct PauseAgentsTool {
    inner: Weak<RuntimeInner>,
    /// `true` for `resume_agents`; the two share every argument and every check.
    resume: bool,
}

#[async_trait]
impl Tool for PauseAgentsTool {
    fn spec(&self) -> ToolSpec {
        let (name, verb, description) = if self.resume {
            (
                RESUME_AGENTS_TOOL,
                "Resume",
                "Let paused subagents continue. They pick up at the next iteration, with their \
                 conversation intact.",
            )
        } else {
            (
                PAUSE_AGENTS_TOOL,
                "Pause",
                "Pause subagents so they stop starting new turns. The turn in flight finishes \
                 first - that is the only point where stopping is free - and the agent then parks \
                 until you resume it, message it, or kill it. A paused agent still holds its slot.",
            )
        };
        ToolSpec {
            name: name.to_string(),
            description: format!(
                "{verb} subagents. {description} Omit `agents` to apply to every subagent you \
                 started, including ones they started."
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agents": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Agent ids. Omit to apply to everything you started."
                    }
                },
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let requested = match parse_agent_ids(&request, "agents") {
            Ok(ids) => ids,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let session_id = ctx.approvals.session_id.0.clone();
        let caller = caller_id(&ctx);
        let targets = match resolve_targets(&inner, &session_id, caller.as_deref(), &requested) {
            Ok(targets) => targets,
            Err(e) => return ToolOutcome::err(e.message()),
        };
        let verb = if self.resume { "resumed" } else { "paused" };
        if targets.is_empty() {
            return ToolOutcome::ok(format!(
                "nothing to {verb}: {} has no live subagents",
                caller_label(&ctx)
            ));
        }
        let by = caller
            .clone()
            .unwrap_or_else(|| "the main agent".to_string());
        let mut lines = Vec::new();
        for agent in &targets {
            // Keep one per-agent guard across the live flag, CAS, fallback read
            // and event. Runtime::control_agent uses the same guard for HTTP, so
            // opposing pause/resume calls cannot cross their store transitions.
            let control = match inner.hub.control(&agent.id.0).await {
                Ok(control) => control,
                Err(e) => {
                    lines.push(format!("`{}`: {}", agent.id.0, e.message()));
                    continue;
                }
            };
            let result = if self.resume {
                control.resume(&by)
            } else {
                control.pause(&by)
            };
            match result {
                Ok(status) => match mark_paused(&inner, control.live(), !self.resume, status).await
                {
                    Ok(actual) => {
                        let requested = if self.resume {
                            AgentStatus::Running
                        } else {
                            AgentStatus::Paused
                        };
                        if actual == requested {
                            ctx.emit(Event::AgentPaused {
                                agent_id: agent.id.0.clone(),
                                paused: !self.resume,
                                by: by.clone(),
                            });
                            lines.push(format!("`{}` {verb}", agent.id.0));
                        } else {
                            lines.push(format!(
                                "`{}` is {}; the {verb} transition did not apply because its persisted state won the race",
                                agent.id.0,
                                actual.as_str()
                            ));
                        }
                    }
                    Err(e) => lines.push(format!(
                        "`{}`: {verb} in the live roster but {e}",
                        agent.id.0
                    )),
                },
                Err(e) => lines.push(format!("`{}`: {}", agent.id.0, e.message())),
            }
        }
        ToolOutcome {
            ok: true,
            content: lines.join("\n"),
            truncated: false,
            meta: Some(serde_json::json!({ "agents": targets.len(), "resumed": self.resume })),
            images: Vec::new(),
        }
    }
}

/// Persist a pause/resume without replacing any other agent fields. The returned
/// status is authoritative: when a terminal write wins, it is returned instead
/// of the requested live state.
async fn mark_paused(
    inner: &Arc<RuntimeInner>,
    agent: &Arc<LiveAgent>,
    paused: bool,
    status: AgentStatus,
) -> Result<AgentStatus> {
    // Resuming an already-cancelled loop must not turn its row back into a live
    // one while finalization is still unwinding.
    if !paused && status == AgentStatus::Cancelled {
        return Ok(AgentStatus::Cancelled);
    }
    let (expected, requested) = if paused {
        (AgentStatus::Running.as_str(), AgentStatus::Paused)
    } else {
        (AgentStatus::Paused.as_str(), AgentStatus::Running)
    };
    let store = inner.deps.store.clone();
    let changed = store
        .transition_agent_status(&agent.id.0, &[expected], requested.as_str(), now_ms())
        .await?;
    if changed {
        return Ok(requested);
    }

    // A terminal full-row upsert may have landed after the hub control check.
    // Conditional UPDATE deliberately loses that race; return the persisted
    // state instead of resurrecting a settled agent with a whole-row write.
    let record = store
        .get_agent(&agent.id.0)
        .await?
        .ok_or_else(|| {
            kc_core::error::Error::NotFound(format!("agent {} no longer exists", agent.id.0))
        })?
        .agent;
    AgentStatus::parse(&record.status).ok_or_else(|| {
        kc_core::error::Error::Store(format!(
            "agent {} has unknown persisted status `{}`",
            agent.id.0, record.status
        ))
    })
}

pub(crate) struct KillAgentsTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for KillAgentsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: KILL_AGENTS_TOOL.to_string(),
            description: "Stop subagents and everything they started. Their work is not lost: the \
                          transcript and any summary produced so far stay readable, and their \
                          status becomes `cancelled`. Omit `agents` to stop every subagent you \
                          started."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "agents": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Agent ids. Omit to stop everything you started."
                    }
                },
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let requested = match parse_agent_ids(&request, "agents") {
            Ok(ids) => ids,
            Err(e) => return ToolOutcome::err(e.to_string()),
        };
        let session_id = ctx.approvals.session_id.0.clone();
        let caller = caller_id(&ctx);
        let targets = match resolve_targets(&inner, &session_id, caller.as_deref(), &requested) {
            Ok(targets) => targets,
            Err(e) => return ToolOutcome::err(e.message()),
        };
        if targets.is_empty() {
            return ToolOutcome::ok(format!(
                "nothing to kill: {} has no live subagents",
                caller_label(&ctx)
            ));
        }
        // Include each target's own subtree: killing a parent that spawned others
        // must not orphan children that outlive it with nobody to collect them.
        let mut ids: Vec<String> = Vec::new();
        for agent in &targets {
            ids.push(agent.id.0.clone());
            for child in inner.hub.descendants_of(&session_id, Some(&agent.id.0)) {
                ids.push(child.id.0.clone());
            }
        }
        ids.sort();
        ids.dedup();
        let by = caller
            .clone()
            .unwrap_or_else(|| "the main agent".to_string());
        let killed = inner.hub.kill(&session_id, &ids, &by);
        ToolOutcome {
            ok: !killed.is_empty(),
            content: format!(
                "asked {} agent{} to stop: {}\n\nThey finish unwinding in a moment; their status \
                 becomes `cancelled` and their transcripts stay readable.",
                killed.len(),
                if killed.len() == 1 { "" } else { "s" },
                killed.join(", ")
            ),
            truncated: false,
            meta: Some(serde_json::json!({ "killed": killed })),
            images: Vec::new(),
        }
    }
}

pub(crate) struct ReportStatusTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for ReportStatusTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: REPORT_STATUS_TOOL.to_string(),
            description:
                "Say what you are doing and how far along you are, so your parent and the \
                          Agents view can see it without reading your whole transcript. Advisory: \
                          nothing verifies it, so report progress honestly or omit it."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "note": {
                        "type": "string",
                        "description": "One line: what you are on now, or what is blocking you."
                    },
                    "progress": {
                        "type": "number",
                        "description": "Fraction complete, 0..1. Omit when you cannot honestly say."
                    }
                },
                "required": ["note"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::ReadOnly
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        let Some(agent_id) = caller_id(&ctx) else {
            return ToolOutcome::err(
                "`report_status` is for a subagent; the main agent reports by answering",
            );
        };
        let Some(note) = request
            .opt_str("note")
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return ToolOutcome::err("`note` may not be empty");
        };
        let note = note.to_string();
        let progress = match request.arguments.get("progress") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => match value.as_f64() {
                Some(value) if (0.0..=1.0).contains(&value) => Some(value as f32),
                _ => {
                    return ToolOutcome::err("`progress` must be a number between 0 and 1");
                }
            },
        };

        inner.hub.note(&agent_id, note.clone(), progress);

        // Persist only the advisory fields. A full-row read/upsert could replay a
        // stale running/paused state over a concurrent control or terminal write.
        // The store's narrow UPDATE deliberately knows nothing about those fields.
        match inner
            .deps
            .store
            .update_agent_note(&agent_id, &note, progress, now_ms())
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(agent = %agent_id, "could not persist a status note: agent row no longer exists")
            }
            Err(e) => {
                tracing::warn!(agent = %agent_id, error = %e, "could not persist a status note")
            }
        }

        ctx.emit(Event::AgentNote {
            agent_id: agent_id.clone(),
            note: note.clone(),
            progress,
        });

        ToolOutcome {
            ok: true,
            content: match progress {
                Some(progress) => {
                    format!("recorded: {note} ({:.0}% complete)", progress * 100.0)
                }
                None => format!("recorded: {note}"),
            },
            truncated: false,
            meta: Some(serde_json::json!({ "note": note, "progress": progress })),
            images: Vec::new(),
        }
    }
}

/// Turn subagent git worktrees on or off, for one session or the whole install.
///
/// This is the middle and outermost of the three scopes the feature has. The
/// narrowest - "use worktrees for this round" - is the `worktree` argument on
/// `spawn_agent`/`spawn_agents`, which needs no state at all. This tool owns the
/// other two: the session's own override (`scope: "session"`, the default) and
/// the install-wide `subagents.worktrees` default the settings screen shows.
///
/// Only the top-level agent gets it. [`child_tool_registry`] drops it for every
/// child at every depth: a subagent deciding how its *siblings* are spawned, or
/// what the whole install does, was never delegated to it.
pub(crate) struct SubagentWorktreesTool {
    inner: Weak<RuntimeInner>,
}

#[async_trait]
impl Tool for SubagentWorktreesTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: SUBAGENT_WORKTREES_TOOL.to_string(),
            description: "Turn git worktrees for subagents on or off, and say at which scope. \
                          `scope: \"session\"` (the default) changes only this conversation - \
                          'use worktrees for the rest of our session'. `scope: \"global\"` changes \
                          the install-wide `subagents.worktrees` default that the settings screen \
                          shows. For a single round of subagents, pass `worktree: true` or \
                          `false` to `spawn_agent`/`spawn_agents` instead of calling this. The \
                          change applies to the next spawn: nothing restarts, and children \
                          already running keep the tree they started in."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["on", "off", "inherit"],
                        "description": "`on`: every subagent gets its own git checkout. `off`: subagents share the parent's working tree. `inherit` (session scope only) clears this session's override and follows the install default again."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["session", "global"],
                        "description": "`session` (default) changes this conversation only; `global` changes the install-wide default."
                    }
                },
                "required": ["mode"],
                "additionalProperties": false
            }),
            origin: kc_core::tool::ToolOrigin::Builtin,
        }
    }

    fn class(&self) -> ToolClass {
        ToolClass::Delegate
    }

    async fn invoke(&self, request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
        let Some(inner) = self.inner.upgrade() else {
            return ToolOutcome::err("the runtime is shutting down");
        };
        // `on`/`off`/`inherit` is the whole vocabulary; every synonym here is one
        // a model reaches for when a user says "turn it off" or "clear that".
        let mode =
            match request
                .opt_str("mode")
                .map(|value| value.trim().to_ascii_lowercase())
            {
                Some(mode) if matches!(mode.as_str(), "on" | "enable" | "enabled" | "true") => {
                    Some(true)
                }
                Some(mode) if matches!(mode.as_str(), "off" | "disable" | "disabled" | "false") => {
                    Some(false)
                }
                Some(mode)
                    if matches!(
                        mode.as_str(),
                        "inherit" | "default" | "clear" | "reset" | "unset"
                    ) =>
                {
                    None
                }
                Some(other) => {
                    return ToolOutcome::err(format!(
                        "`mode` must be `on`, `off` or `inherit`, not `{other}`"
                    ))
                }
                None => return ToolOutcome::err(
                    "`mode` is required: `on`, `off`, or `inherit` to follow the install default",
                ),
            };
        let scope = request
            .opt_str("scope")
            .map(|value| value.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "session".to_string());
        let global = ctx.settings.subagent_config().await.worktrees;

        match scope.as_str() {
            "session" => {
                let session_id = ctx.approvals.session_id.clone();
                if let Err(e) = inner
                    .deps
                    .store
                    .set_session_worktrees(&session_id.0, mode)
                    .await
                {
                    return ToolOutcome::err(format!(
                        "could not change this session's worktree override: {e}"
                    ));
                }
                let effective = mode.unwrap_or(global);
                let what = match mode {
                    Some(true) => "on for this session".to_string(),
                    Some(false) => "off for this session".to_string(),
                    None => format!(
                        "following the install default ({})",
                        if global { "on" } else { "off" }
                    ),
                };
                ToolOutcome {
                    ok: true,
                    content: format!(
                        "worktrees for subagents: {what}. New subagents in this session {}.\n{}\n\n\
                         The install default is unchanged ({}). A single spawn can still override \
                         this either way with its `worktree` argument.",
                        if effective {
                            "get their own git checkout"
                        } else {
                            "share this working tree"
                        },
                        worktree_explanation(effective),
                        if global { "on" } else { "off" },
                    ),
                    truncated: false,
                    meta: Some(serde_json::json!({
                        "scope": "session",
                        "session_worktrees": mode,
                        "global": global,
                        "effective": effective,
                    })),
                    images: Vec::new(),
                }
            }
            "global" => {
                let Some(on) = mode else {
                    return ToolOutcome::err(
                        "the install-wide default has no `inherit`: pass `mode: \"on\"` or `\"off\"` \
                         to set it, or `scope: \"session\"` with `mode: \"inherit\"` to clear this \
                         session's override and follow it",
                    );
                };
                if let Err(e) = ctx.settings.set_subagent_worktrees(on).await {
                    return ToolOutcome::err(format!("could not change subagents.worktrees: {e}"));
                }
                // The session's own override still wins over this, so say which
                // one a spawn in *this* conversation will actually use.
                let session = inner
                    .deps
                    .store
                    .get_session(&ctx.approvals.session_id.0)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|session| session.worktrees);
                let effective = session.unwrap_or(on);
                let mut content = format!(
                    "worktrees for subagents: {} for the whole install (subagents.worktrees). \
                     New subagents in every session follow it unless that session overrides it.\n{}",
                    if on { "on" } else { "off" },
                    worktree_explanation(on),
                );
                if session.is_some() {
                    content.push_str(&format!(
                        "\n\nThis session has its own override ({}), so its subagents {} until you \
                         clear it with `subagent_worktrees` `{{\"mode\": \"inherit\"}}`.",
                        if effective { "on" } else { "off" },
                        if effective {
                            "keep getting their own checkout"
                        } else {
                            "keep sharing this working tree"
                        },
                    ));
                }
                ToolOutcome {
                    ok: true,
                    content,
                    truncated: false,
                    meta: Some(serde_json::json!({
                        "scope": "global",
                        "global": on,
                        "session_worktrees": session,
                        "effective": effective,
                    })),
                    images: Vec::new(),
                }
            }
            other => ToolOutcome::err(format!(
                "`scope` must be `session` or `global`, not `{other}`"
            )),
        }
    }
}

/// The "what that means" sentence the worktree tools answer with.
///
/// Both scopes need it: turning isolation on is not self-explanatory, and the
/// part that surprises people is that nothing is merged for them.
fn worktree_explanation(on: bool) -> &'static str {
    if on {
        "Each child gets a detached checkout under `.karucode/worktrees/<agent-id>`, started from \
         your working tree as it is right now; its edits never touch yours. A child that changed \
         files has them committed onto `karucode/subagent/<agent-id>` when it finishes - nothing \
         is merged for you, and its summary names the branch. It needs a git repository with at \
         least one commit: in a plain directory the spawn is refused rather than silently shared."
    } else {
        "Children run in this working tree, so their edits are visible immediately and survive \
         completion. Two children editing the same file will interleave."
    }
}

/// Convenience for embedders that build their own registry: install the
/// runtime built-ins into `registry`.
pub fn install_builtins(registry: &mut ToolRegistry, runtime: &Arc<crate::Runtime>) {
    install_delegate_tools(runtime.shared(), registry);
}

/// The wall-clock budget of one child, shared with the roster so a stale agent is
/// visible rather than silent.
pub fn child_timeout_secs(cfg: &kc_core::config::SubagentConfig) -> u64 {
    cfg.timeout_secs.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::ids::{RunId, SessionId};
    use kc_core::paths::Workspace;
    use kc_core::store::Store as _;
    use std::path::Path;

    fn control_test_ctx(
        inner: &Arc<RuntimeInner>,
        session_id: &str,
        agent_id: Option<&str>,
    ) -> (ToolCtx, Arc<Mutex<Vec<Event>>>) {
        control_test_ctx_for_run(inner, session_id, "r_test", agent_id)
    }

    fn control_test_ctx_for_run(
        inner: &Arc<RuntimeInner>,
        session_id: &str,
        run_id: &str,
        agent_id: Option<&str>,
    ) -> (ToolCtx, Arc<Mutex<Vec<Event>>>) {
        let session = SessionId::from_string(session_id);
        let run = RunId::from_string(run_id);
        let agent = agent_id.map(|id| AgentId(id.to_string()));
        let gate: Arc<dyn kc_core::tool::ApprovalGate> = Arc::new(RuntimeApprovalGate::new(
            Arc::clone(inner),
            session.clone(),
            run.clone(),
            agent.clone(),
            CancellationToken::new(),
            agent_id.map_or(0, |_| 1),
        ));
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        (
            ToolCtx {
                workspace: Arc::new(Workspace::new(std::env::temp_dir()).expect("temp workspace")),
                cancel: CancellationToken::new(),
                approvals: kc_core::tool::ApprovalHandle::new(gate, session, run, agent),
                events: Arc::new(move |event| {
                    recorded.lock().expect("recorded events").push(event);
                }),
                settings: Arc::clone(&inner.deps.settings),
                depth: agent_id.map_or(0, |_| 1),
                supports_images: false,
                checkpoints: None,
            },
            events,
        )
    }

    fn tool_request(name: &str, arguments: serde_json::Value) -> ToolCallRequest {
        ToolCallRequest {
            call_id: "test-call".into(),
            name: name.into(),
            arguments,
        }
    }

    #[test]
    fn limiter_counts_and_releases() {
        let limiter = Arc::new(SubagentLimiter::new(Default::default()));
        let cancel = CancellationToken::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let a = limiter.acquire(1, &cancel).await.unwrap();
            assert_eq!(limiter.active(), 1);
            assert_eq!(limiter.peak(), 1);
            let blocked = Arc::clone(&limiter);
            let c2 = cancel.clone();
            let waiter = tokio::spawn(async move {
                let permit = blocked.acquire(1, &c2).await;
                permit.is_some()
            });
            tokio::time::sleep(Duration::from_millis(20)).await;
            assert!(!waiter.is_finished());
            drop(a);
            assert_eq!(limiter.active(), 0);
            assert!(waiter.await.unwrap());
            assert_eq!(limiter.peak(), 1);
        });
    }

    #[test]
    fn limiter_gives_up_on_cancel() {
        let limiter = Arc::new(SubagentLimiter::new(Default::default()));
        let cancel = CancellationToken::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let _held = limiter.acquire(1, &cancel).await.unwrap();
            let c2 = cancel.clone();
            let blocked = Arc::clone(&limiter);
            let waiter = tokio::spawn(async move { blocked.acquire(1, &c2).await.is_some() });
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
            assert!(!waiter.await.unwrap());
        });
    }

    #[test]
    fn try_acquire_never_waits() {
        let limiter = Arc::new(SubagentLimiter::new(Default::default()));
        let first = limiter.try_acquire(1).expect("a free slot");
        assert!(
            limiter.try_acquire(1).is_none(),
            "a background spawn must be refused, not parked"
        );
        drop(first);
        assert!(limiter.try_acquire(1).is_some());
    }

    /// `0` is the "no ceiling" spelling, and the two paths that read it have to
    /// agree: a background spawn (which must never park its parent) and a
    /// blocking one (which may). Reading 0 as "allow nothing" here would turn the
    /// widest setting an operator can choose into a harness that refuses every
    /// spawn - the exact inverse of the documented contract.
    #[test]
    fn a_zero_ceiling_grants_every_slot() {
        let limiter = Arc::new(SubagentLimiter::new(Default::default()));
        let mut permits = Vec::new();
        for _ in 0..500 {
            permits.push(
                limiter
                    .try_acquire(0)
                    .expect("no ceiling means a slot is always free"),
            );
        }
        assert_eq!(limiter.active(), 500, "slots are still counted honestly");
        assert_eq!(limiter.peak(), 500);

        // The blocking path agrees with the immediate one, and does not depend on
        // a permit being released first.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let cancel = CancellationToken::new();
        rt.block_on(async {
            let another = limiter
                .acquire(0, &cancel)
                .await
                .expect("the unlimited acquire must not block");
            drop(another);
        });

        drop(permits);
        assert_eq!(limiter.active(), 0, "every permit is released on drop");
    }

    #[tokio::test]
    async fn a_recursive_blocking_spawn_is_refused_at_capacity_instead_of_deadlocking() {
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};

        let settings = subagent_config_for_test(1);
        let task = "level one";
        let spawn = |call_id: &str, task: &str| {
            Script::call(&[(
                call_id,
                SPAWN_AGENT_TOOL,
                &serde_json::json!({ "task": task }).to_string(),
            )])
        };
        let mut h =
            HarnessBuilder::new(vec![spawn("root_spawn", task), Script::text("parent done")])
                .settings(FakeSettings::default().with_subagents(settings))
                .build()
                .await;
        h.factory.key(
            task,
            vec![
                spawn("nested_spawn", "level two"),
                Script::text("child done after the refusal"),
            ],
        );

        // Harness timeouts turn the old wait-for cycle into a bounded failure.
        let events = h.run_to_finish("start the recursive chain").await;
        let spawned = events
            .iter()
            .filter(|event| matches!(event.event, Event::AgentSpawned { .. }))
            .count();
        assert_eq!(spawned, 1, "the at-capacity grandchild must not start");
        let nested = events
            .iter()
            .find_map(|event| match &event.event {
                Event::ToolCallFinished {
                    call_id,
                    ok,
                    output,
                    ..
                } if call_id == "nested_spawn" => Some((*ok, output.clone())),
                _ => None,
            })
            .expect("nested spawn result");
        assert!(!nested.0, "the nested spawn must refuse: {}", nested.1);
        assert!(nested.1.contains("max_concurrent = 1"), "{}", nested.1);
        assert!(nested.1.contains("own ancestry"), "{}", nested.1);
        assert!(h.store.agents().iter().all(|a| a.status == "completed"));
    }

    struct SharedWorkspaceWrite;

    #[async_trait]
    impl Tool for SharedWorkspaceWrite {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "write_file".into(),
                description: "Verify dirty parent reads, then write one child file".into(),
                parameters: serde_json::json!({"type": "object"}),
                origin: kc_core::tool::ToolOrigin::Builtin,
            }
        }

        fn class(&self) -> ToolClass {
            ToolClass::Mutating
        }

        async fn invoke(&self, _request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
            let dirty = match std::fs::read_to_string(ctx.workspace.root().join("dirty.txt")) {
                Ok(text) => text,
                Err(e) => {
                    return ToolOutcome::err(format!("could not read dirty parent file: {e}"))
                }
            };
            let untracked =
                match std::fs::read_to_string(ctx.workspace.root().join("parent-untracked.txt")) {
                    Ok(text) => text,
                    Err(e) => {
                        return ToolOutcome::err(format!(
                            "could not read untracked parent file: {e}"
                        ))
                    }
                };
            ctx.record_checkpoint("child-created.txt", "write_file")
                .await;
            if let Err(e) = std::fs::write(
                ctx.workspace.root().join("child-created.txt"),
                "completed child edit\n",
            ) {
                return ToolOutcome::err(format!("could not write child file: {e}"));
            }
            ToolOutcome::ok(format!("dirty={dirty}; untracked={untracked}"))
        }
    }

    fn git(workspace: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(workspace)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run git for shared-workspace regression");
        assert!(status.success(), "git {args:?} failed");
    }

    /// A tool that reports the tree the child is confined to and leaves an edit
    /// in it, so a test can see both where it ran and what it produced.
    struct WorktreeProbe;

    #[async_trait]
    impl Tool for WorktreeProbe {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "worktree_probe".into(),
                description: "report the child's workspace and write into it".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                origin: Default::default(),
            }
        }

        fn class(&self) -> ToolClass {
            ToolClass::Mutating
        }

        async fn invoke(&self, _request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
            let root = ctx.workspace.root();
            let seen = std::fs::read_to_string(root.join("tracked.txt")).unwrap_or_default();
            if let Err(e) = std::fs::write(root.join("child-file.txt"), "from the child\n") {
                return ToolOutcome::err(format!("could not write child file: {e}"));
            }
            ToolOutcome::ok(format!("root={}; tracked={}", root.display(), seen.trim()))
        }
    }

    fn worktree_test_settings(worktrees: bool) -> kc_core::config::SubagentConfig {
        kc_core::config::SubagentConfig {
            worktrees,
            ..subagent_config_for_test(2)
        }
    }

    fn init_repo_with_commit(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "test@example.invalid"]);
        git(dir, &["config", "user.name", "Test"]);
        std::fs::write(dir.join("tracked.txt"), "committed\n").unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Every stored child row's summary, which is what the parent is shown.
    async fn child_summary(h: &crate::tests_support::Harness) -> String {
        use kc_core::store::Store as _;
        let rows = h.store.list_agents(h.session()).await.unwrap_or_default();
        rows.iter()
            .map(|a| a.summary.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn tool_output(events: &[kc_core::events::EventEnvelope], call_id: &str) -> String {
        events
            .iter()
            .find_map(|event| match &event.event {
                Event::ToolCallFinished {
                    call_id: id,
                    output,
                    ..
                } if id == call_id => Some(output.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no tool result for {call_id}"))
    }

    /// A tool that journals its write the way a real file tool does, so a test can
    /// see which tree the child's checkpoint resolves against.
    struct JournalingProbe;

    #[async_trait]
    impl Tool for JournalingProbe {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "journaling_probe".into(),
                description: "journal an overwrite of a tracked file".into(),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                origin: Default::default(),
            }
        }

        fn class(&self) -> ToolClass {
            ToolClass::Mutating
        }

        async fn invoke(&self, _request: ToolCallRequest, ctx: ToolCtx) -> ToolOutcome {
            // `record_checkpoint` captures the before-image and writes the row, so
            // one call is one journaled write.
            if !ctx.capture_checkpoint("tracked.txt", "write_file").await {
                return ToolOutcome::err("checkpoint could not be captured");
            }
            if let Err(e) = std::fs::write(ctx.workspace.root().join("tracked.txt"), "child edit\n")
            {
                return ToolOutcome::err(format!("could not write: {e}"));
            }
            ToolOutcome::ok("journalled")
        }
    }

    /// An isolated child's journal must belong to its own checkout. Sharing the
    /// parent's would capture the before-image from the child's tree and then
    /// restore it into the parent's on rewind - silently rewriting the parent's
    /// file with a worktree's content.
    #[tokio::test]
    async fn an_isolated_child_journals_against_its_own_checkout() {
        if !git_available() {
            return;
        }
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};
        use kc_core::store::Store as _;
        let child_task = "journal an edit";
        let mut h = HarnessBuilder::new(vec![
            Script::call(&[(
                "root_spawn",
                SPAWN_AGENT_TOOL,
                &serde_json::json!({ "task": child_task }).to_string(),
            )]),
            Script::text("parent done"),
        ])
        .tool(Arc::new(JournalingProbe))
        .settings(FakeSettings::default().with_subagents(worktree_test_settings(true)))
        .build()
        .await;
        h.factory.key(
            child_task,
            vec![
                Script::call(&[("child_journal", "journaling_probe", "{}")]),
                Script::text("child done"),
            ],
        );
        init_repo_with_commit(&h.workspace_dir);

        let events = h.run_to_finish("journalling in a worktree").await;
        h.wait_idle().await;
        assert_eq!(tool_output(&events, "child_journal").trim(), "journalled");

        // The child's write is in its checkout, which is gone; the parent's file
        // never changed, so a rewind into it would be the bug this guards.
        assert_eq!(
            std::fs::read_to_string(h.workspace_dir.join("tracked.txt")).unwrap(),
            "committed\n",
            "the parent's file must be exactly as it was"
        );
        let rows = h.store.list_checkpoints(h.session(), 0).await.unwrap();
        assert_eq!(
            rows.len(),
            1,
            "the child's write is journaled once: {rows:?}"
        );
        assert_eq!(rows[0].path, "tracked.txt");
        assert!(rows[0].agent_id.is_some(), "the row names the child");

        // Rewinding the parent must not resurrect the checkout's content into the
        // parent tree: the child's journal is scoped to a directory that no longer
        // exists, so the parent's "committed" state is untouched either way.
        let prompt = h
            .session_messages()
            .await
            .into_iter()
            .find(|m| m.role == "user" && m.agent_id.is_none())
            .expect("parent prompt");
        let _ = h.runtime.rewind(h.session(), prompt.id, true, true).await;
        assert_eq!(
            std::fs::read_to_string(h.workspace_dir.join("tracked.txt")).unwrap(),
            "committed\n",
            "a rewind must never write a worktree's content into the parent tree"
        );
    }

    /// With the setting off - the default - a child works in the parent's own
    /// tree and its edits are simply there afterwards.
    #[tokio::test]
    async fn without_the_setting_a_child_still_works_in_the_parent_tree() {
        if !git_available() {
            return;
        }
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};
        let child_task = "probe the tree";
        let mut h = HarnessBuilder::new(vec![
            Script::call(&[(
                "root_spawn",
                SPAWN_AGENT_TOOL,
                &serde_json::json!({ "task": child_task }).to_string(),
            )]),
            Script::text("parent done"),
        ])
        .tool(Arc::new(WorktreeProbe))
        .settings(FakeSettings::default().with_subagents(worktree_test_settings(false)))
        .build()
        .await;
        h.factory.key(
            child_task,
            vec![
                Script::call(&[("child_probe", "worktree_probe", "{}")]),
                Script::text("child done"),
            ],
        );
        init_repo_with_commit(&h.workspace_dir);
        std::fs::write(h.workspace_dir.join("tracked.txt"), "parent dirty\n").unwrap();

        let events = h.run_to_finish("exercise the default tree").await;
        h.wait_idle().await;
        let probe = tool_output(&events, "child_probe");
        assert!(probe.contains("tracked=parent dirty"), "{probe}");
        assert!(
            !probe.contains(crate::worktree::WORKTREE_DIR),
            "the default must not create a worktree: {probe}"
        );
        assert_eq!(
            std::fs::read_to_string(h.workspace_dir.join("child-file.txt")).unwrap(),
            "from the child\n",
            "a shared child's edit lands in the parent tree"
        );
    }

    /// With the setting on, the child runs in its own checkout, its edit never
    /// touches the parent's working tree, and its work is handed back on a branch.
    #[tokio::test]
    async fn with_the_setting_a_child_works_in_its_own_worktree() {
        if !git_available() {
            return;
        }
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};
        let child_task = "probe the isolated tree";
        let mut h = HarnessBuilder::new(vec![
            Script::call(&[(
                "root_spawn",
                SPAWN_AGENT_TOOL,
                &serde_json::json!({ "task": child_task }).to_string(),
            )]),
            Script::text("parent done"),
        ])
        .tool(Arc::new(WorktreeProbe))
        .settings(FakeSettings::default().with_subagents(worktree_test_settings(true)))
        .build()
        .await;
        h.factory.key(
            child_task,
            vec![
                Script::call(&[("child_probe", "worktree_probe", "{}")]),
                Script::text("child done"),
            ],
        );
        init_repo_with_commit(&h.workspace_dir);
        std::fs::write(h.workspace_dir.join("tracked.txt"), "parent dirty\n").unwrap();

        let events = h.run_to_finish("exercise the isolated tree").await;
        h.wait_idle().await;

        let probe = tool_output(&events, "child_probe");
        assert!(
            probe.contains(crate::worktree::WORKTREE_DIR),
            "the child must work in its own checkout: {probe}"
        );
        assert!(
            probe.contains("tracked=parent dirty"),
            "the child must still see the parent's uncommitted work: {probe}"
        );
        assert!(
            !h.workspace_dir.join("child-file.txt").exists(),
            "an isolated child's edit must not land in the parent's working tree"
        );
        assert_eq!(
            std::fs::read_to_string(h.workspace_dir.join("tracked.txt")).unwrap(),
            "parent dirty\n",
            "the parent's own file is untouched"
        );

        // The child's work is not lost: its summary names the branch it went to.
        let summary = child_summary(&h).await;
        assert!(
            summary.contains("[worktree]") && summary.contains(crate::worktree::BRANCH_PREFIX),
            "the child's summary must hand its branch back: {summary:?}"
        );
        let branches = std::process::Command::new("git")
            .args([
                "branch",
                "--list",
                &format!("{}/*", crate::worktree::BRANCH_PREFIX),
            ])
            .current_dir(&h.workspace_dir)
            .output()
            .expect("list branches");
        let listed = String::from_utf8_lossy(&branches.stdout);
        assert!(
            listed.contains(crate::worktree::BRANCH_PREFIX),
            "the child's work must be on its own branch: {listed:?}"
        );
    }

    /// A non-git workspace with the setting on is refused rather than silently
    /// running the child in the shared tree.
    #[tokio::test]
    async fn a_worktree_spawn_outside_a_repository_is_refused_by_name() {
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};
        let mut h = HarnessBuilder::new(vec![
            Script::call(&[(
                "root_spawn",
                SPAWN_AGENT_TOOL,
                &serde_json::json!({ "task": "should not start" }).to_string(),
            )]),
            Script::text("parent done"),
        ])
        .settings(FakeSettings::default().with_subagents(worktree_test_settings(true)))
        .build()
        .await;

        let events = h
            .run_to_finish("try an isolated child in a plain directory")
            .await;
        h.wait_idle().await;
        let result = tool_output(&events, "root_spawn");
        assert!(
            result.contains("subagents.worktrees"),
            "the refusal must name the setting: {result}"
        );
        assert!(
            result.contains("not inside a git repository"),
            "the refusal must say why: {result}"
        );
    }

    /// The session's own override isolates its children even though the install
    /// default says to share: this is "use worktrees for the rest of our session".
    #[tokio::test]
    async fn a_session_override_isolates_children_with_the_install_default_off() {
        if !git_available() {
            return;
        }
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};
        use kc_core::store::Store as _;
        let child_task = "probe the session's own tree";
        let mut h = HarnessBuilder::new(vec![
            Script::call(&[(
                "root_spawn",
                SPAWN_AGENT_TOOL,
                &serde_json::json!({ "task": child_task }).to_string(),
            )]),
            Script::text("parent done"),
        ])
        .tool(Arc::new(WorktreeProbe))
        .settings(FakeSettings::default().with_subagents(worktree_test_settings(false)))
        .build()
        .await;
        h.factory.key(
            child_task,
            vec![
                Script::call(&[("child_probe", "worktree_probe", "{}")]),
                Script::text("child done"),
            ],
        );
        init_repo_with_commit(&h.workspace_dir);
        h.store
            .set_session_worktrees(h.session(), Some(true))
            .await
            .expect("pin the session to worktrees");

        let events = h.run_to_finish("isolate this session's children").await;
        h.wait_idle().await;

        let probe = tool_output(&events, "child_probe");
        assert!(
            probe.contains(crate::worktree::WORKTREE_DIR),
            "the session override must give the child its own checkout: {probe}"
        );
        assert!(
            !h.workspace_dir.join("child-file.txt").exists(),
            "an isolated child's edit must not land in the parent's working tree"
        );
    }

    /// The other direction: a session that turned isolation off keeps its children
    /// in the shared tree even after the install default was turned on.
    #[tokio::test]
    async fn a_session_override_keeps_children_shared_with_the_install_default_on() {
        if !git_available() {
            return;
        }
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};
        use kc_core::store::Store as _;
        let child_task = "probe the shared tree anyway";
        let mut h = HarnessBuilder::new(vec![
            Script::call(&[(
                "root_spawn",
                SPAWN_AGENT_TOOL,
                &serde_json::json!({ "task": child_task }).to_string(),
            )]),
            Script::text("parent done"),
        ])
        .tool(Arc::new(WorktreeProbe))
        .settings(FakeSettings::default().with_subagents(worktree_test_settings(true)))
        .build()
        .await;
        h.factory.key(
            child_task,
            vec![
                Script::call(&[("child_probe", "worktree_probe", "{}")]),
                Script::text("child done"),
            ],
        );
        init_repo_with_commit(&h.workspace_dir);
        h.store
            .set_session_worktrees(h.session(), Some(false))
            .await
            .expect("pin the session to the shared tree");

        let events = h.run_to_finish("share this session's tree").await;
        h.wait_idle().await;

        let probe = tool_output(&events, "child_probe");
        assert!(
            !probe.contains(crate::worktree::WORKTREE_DIR),
            "a session override to off must win over the install default: {probe}"
        );
        assert_eq!(
            std::fs::read_to_string(h.workspace_dir.join("child-file.txt")).unwrap(),
            "from the child\n",
            "a shared child's edit lands in the parent tree"
        );
    }

    /// One round only: the `worktree` argument isolates this spawn and leaves the
    /// session's stored choice alone, which is what "this round of subagents" is.
    #[tokio::test]
    async fn a_worktree_argument_isolates_one_round_and_is_not_remembered() {
        if !git_available() {
            return;
        }
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};
        use kc_core::store::Store as _;
        let child_task = "probe this round's tree";
        let mut h = HarnessBuilder::new(vec![
            Script::call(&[(
                "root_spawn",
                SPAWN_AGENT_TOOL,
                &serde_json::json!({ "task": child_task, "worktree": true }).to_string(),
            )]),
            Script::text("parent done"),
        ])
        .tool(Arc::new(WorktreeProbe))
        .settings(FakeSettings::default().with_subagents(worktree_test_settings(false)))
        .build()
        .await;
        h.factory.key(
            child_task,
            vec![
                Script::call(&[("child_probe", "worktree_probe", "{}")]),
                Script::text("child done"),
            ],
        );
        init_repo_with_commit(&h.workspace_dir);

        let events = h.run_to_finish("isolate this one round").await;
        h.wait_idle().await;

        let probe = tool_output(&events, "child_probe");
        assert!(
            probe.contains(crate::worktree::WORKTREE_DIR),
            "the per-call argument must isolate the child: {probe}"
        );
        assert!(
            !h.workspace_dir.join("child-file.txt").exists(),
            "an isolated child's edit must not land in the parent's working tree"
        );
        let session = h.store.get_session(h.session()).await.unwrap().unwrap();
        assert_eq!(
            session.worktrees, None,
            "a per-round argument must not become the session's setting"
        );
    }

    /// The tool is the "rest of our session" path: what it records is what the
    /// next spawn uses, with no restart and no global change.
    #[tokio::test]
    async fn the_worktrees_tool_sets_the_session_scope_for_later_spawns() {
        if !git_available() {
            return;
        }
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};
        use kc_core::store::Store as _;
        let child_task = "probe the tool's tree";
        let mut h = HarnessBuilder::new(vec![
            Script::call(&[(
                "set_worktrees",
                SUBAGENT_WORKTREES_TOOL,
                &serde_json::json!({ "mode": "on" }).to_string(),
            )]),
            Script::call(&[(
                "root_spawn",
                SPAWN_AGENT_TOOL,
                &serde_json::json!({ "task": child_task }).to_string(),
            )]),
            Script::text("parent done"),
        ])
        .tool(Arc::new(WorktreeProbe))
        .settings(FakeSettings::default().with_subagents(worktree_test_settings(false)))
        .build()
        .await;
        h.factory.key(
            child_task,
            vec![
                Script::call(&[("child_probe", "worktree_probe", "{}")]),
                Script::text("child done"),
            ],
        );
        init_repo_with_commit(&h.workspace_dir);

        let events = h.run_to_finish("turn worktrees on for this session").await;
        h.wait_idle().await;

        let answer = tool_output(&events, "set_worktrees");
        assert!(
            answer.contains("on for this session"),
            "the tool must say what it set: {answer}"
        );
        let session = h.store.get_session(h.session()).await.unwrap().unwrap();
        assert_eq!(session.worktrees, Some(true));
        let probe = tool_output(&events, "child_probe");
        assert!(
            probe.contains(crate::worktree::WORKTREE_DIR),
            "the spawn after the tool must use the session override: {probe}"
        );
    }

    /// A subagent is never offered the switch: it can neither change how its
    /// siblings are spawned nor what the install does.
    #[tokio::test]
    async fn a_child_never_sees_the_worktrees_tool() {
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};
        let h = HarnessBuilder::new(vec![Script::text("parent done")])
            .settings(FakeSettings::default().with_subagents(worktree_test_settings(false)))
            .build()
            .await;

        assert!(
            h.runtime.tools().get(SUBAGENT_WORKTREES_TOOL).is_some(),
            "the top-level agent has it"
        );
        // Recursion on: the deepest child that may delegate still does not get it.
        let mut cfg = worktree_test_settings(false);
        cfg.allow_recursive = true;
        cfg.max_depth = 3;
        let child = child_tool_registry(h.runtime.shared(), 1, &cfg);
        assert!(
            child.get(SUBAGENT_WORKTREES_TOOL).is_none(),
            "a subagent must not be able to change isolation for the session or the install"
        );
        assert!(
            child.get(SPAWN_AGENT_TOOL).is_some(),
            "the same registry still lets a recursive child spawn"
        );
    }

    #[tokio::test]
    async fn a_completed_child_shares_dirty_parent_state_and_rewinds_the_same_tree() {
        use crate::tests_support::{FakeSettings, HarnessBuilder, Script};
        use kc_core::checkpoint::UndoOutcome;
        use kc_core::store::Store as _;

        let child_task = "edit the live parent workspace";
        let spawn = Script::call(&[(
            "root_spawn",
            SPAWN_AGENT_TOOL,
            &serde_json::json!({ "task": child_task }).to_string(),
        )]);
        let mut h = HarnessBuilder::new(vec![spawn, Script::text("parent done")])
            .tool(Arc::new(SharedWorkspaceWrite))
            .settings(FakeSettings::default().with_subagents(subagent_config_for_test(2)))
            .build()
            .await;
        h.factory.key(
            child_task,
            vec![
                Script::call(&[("child_write", "write_file", "{}")]),
                Script::text("child completed"),
            ],
        );

        // A real HEAD is required to reproduce the old automatic-worktree path.
        git(&h.workspace_dir, &["init", "-q"]);
        std::fs::write(h.workspace_dir.join("dirty.txt"), "committed\n").unwrap();
        git(&h.workspace_dir, &["add", "dirty.txt"]);
        git(
            &h.workspace_dir,
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        );
        std::fs::write(h.workspace_dir.join("dirty.txt"), "parent dirty\n").unwrap();
        std::fs::write(
            h.workspace_dir.join("parent-untracked.txt"),
            "parent untracked\n",
        )
        .unwrap();

        let events = h.run_to_finish("exercise the shared child workspace").await;
        h.wait_idle().await;
        let child_write = events
            .iter()
            .find_map(|event| match &event.event {
                Event::ToolCallFinished {
                    call_id,
                    ok,
                    output,
                    ..
                } if call_id == "child_write" => Some((*ok, output.clone())),
                _ => None,
            })
            .expect("child write result");
        assert!(child_write.0, "{}", child_write.1);
        assert!(
            child_write.1.contains("dirty=parent dirty"),
            "{}",
            child_write.1
        );
        assert!(
            child_write.1.contains("untracked=parent untracked"),
            "{}",
            child_write.1
        );
        assert_eq!(
            std::fs::read_to_string(h.workspace_dir.join("child-created.txt")).unwrap(),
            "completed child edit\n",
            "the edit must survive child completion"
        );

        let rows = h.store.list_checkpoints(h.session(), 0).await.unwrap();
        assert_eq!(rows.len(), 1, "the shared child write is journaled once");
        assert_eq!(rows[0].path, "child-created.txt");
        assert!(rows[0].agent_id.is_some(), "the row names the child");
        let prompt = h
            .session_messages()
            .await
            .into_iter()
            .find(|m| m.role == "user" && m.agent_id.is_none())
            .expect("parent prompt");
        let outcome = h
            .runtime
            .rewind(h.session(), prompt.id, true, true)
            .await
            .expect("rewind the shared child checkpoint");
        assert_eq!(outcome.outcomes, vec![UndoOutcome::Removed]);
        assert!(
            !h.workspace_dir.join("child-created.txt").exists(),
            "the journal and child tools must resolve the same parent tree"
        );
    }

    fn status_record(id: &str, status: AgentStatus) -> AgentRecord {
        AgentRecord {
            id: id.into(),
            session_id: "s_status".into(),
            parent_run_id: "r_status".into(),
            parent_agent_id: None,
            name: "worker".into(),
            role: "worker".into(),
            task: "work".into(),
            status: status.as_str().into(),
            model: "fake".into(),
            iterations: 0,
            summary: String::new(),
            error: None,
            owned_paths: Vec::new(),
            created_at: 10,
            updated_at: 10,
            finished_at: None,
            background: false,
            note: String::new(),
            progress: None,
        }
    }

    #[tokio::test]
    async fn explicit_messages_stay_in_the_senders_session() {
        let inner = crate::tests_support::test_inner();
        for (id, session, parent) in [
            ("a_parent", "s_status", None),
            ("a_sender", "s_status", Some("a_parent")),
            ("a_sibling", "s_status", Some("a_parent")),
            ("a_foreign", "s_other", None),
        ] {
            let mut record = status_record(id, AgentStatus::Running);
            record.session_id = session.into();
            record.parent_agent_id = parent.map(str::to_string);
            inner.deps.store.upsert_agent(&record).await.unwrap();
            inner.hub.insert(
                &record,
                record.parent_run_id.clone(),
                1,
                CancellationToken::new(),
            );
        }
        let (ctx, events) = control_test_ctx(&inner, "s_status", Some("a_sender"));
        let tool = SendMessageTool {
            inner: Arc::downgrade(&inner),
        };

        let rejected = tool
            .invoke(
                tool_request(
                    SEND_MESSAGE_TOOL,
                    serde_json::json!({"message": "cross-session prompt", "agents": ["a_foreign"]}),
                ),
                ctx.clone(),
            )
            .await;
        assert!(!rejected.ok, "{}", rejected.content);
        assert_eq!(rejected.meta.unwrap()["delivered"], 0);
        assert_eq!(inner.hub.get("a_foreign").unwrap().queued_mail(), 0);
        assert!(
            events.lock().unwrap().is_empty(),
            "rejection emitted an event"
        );

        // Siblings and parents are intentionally still addressable in the same
        // session; the boundary is the sender's ctx session, not an ownership ban
        // on messaging.
        let accepted = tool
            .invoke(
                tool_request(
                    SEND_MESSAGE_TOOL,
                    serde_json::json!({"message": "same session", "agents": ["a_sibling", "a_parent"]}),
                ),
                ctx.clone(),
            )
            .await;
        assert!(accepted.ok, "{}", accepted.content);
        assert_eq!(accepted.meta.unwrap()["delivered"], 2);
        assert_eq!(inner.hub.get("a_sibling").unwrap().queued_mail(), 1);
        assert_eq!(inner.hub.get("a_parent").unwrap().queued_mail(), 1);

        // The omitted-target parent path remains compatible too.
        let parent = tool
            .invoke(
                tool_request(
                    SEND_MESSAGE_TOOL,
                    serde_json::json!({"message": "implicit parent"}),
                ),
                ctx,
            )
            .await;
        assert!(parent.ok, "{}", parent.content);
        assert_eq!(inner.hub.get("a_parent").unwrap().queued_mail(), 2);
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(
                    |event| matches!(event, Event::AgentMessage { state, .. } if state == "queued")
                )
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn send_message_never_counts_a_dropped_33rd_message() {
        let inner = crate::tests_support::test_inner();
        let record = status_record("a_full", AgentStatus::Running);
        let live = inner.hub.insert(
            &record,
            record.parent_run_id.clone(),
            1,
            CancellationToken::new(),
        );
        let (ctx, events) = control_test_ctx(&inner, "s_status", None);
        let tool = SendMessageTool {
            inner: Arc::downgrade(&inner),
        };
        for index in 0..MAX_MAILBOX {
            let outcome = tool
                .invoke(
                    tool_request(
                        SEND_MESSAGE_TOOL,
                        serde_json::json!({"message": format!("m{index}"), "agents": ["a_full"]}),
                    ),
                    ctx.clone(),
                )
                .await;
            assert!(
                outcome.ok,
                "message {} failed: {}",
                index + 1,
                outcome.content
            );
            assert_eq!(outcome.meta.unwrap()["delivered"], 1);
        }
        assert_eq!(events.lock().unwrap().len(), MAX_MAILBOX);

        let rejected = tool
            .invoke(
                tool_request(
                    SEND_MESSAGE_TOOL,
                    serde_json::json!({"message": "dropped-33", "agents": ["a_full"]}),
                ),
                ctx,
            )
            .await;
        assert!(
            !rejected.ok,
            "a full mailbox was acknowledged: {}",
            rejected.content
        );
        assert!(rejected.content.contains("nothing was queued"));
        assert_eq!(rejected.meta.unwrap()["delivered"], 0);
        assert_eq!(live.queued_mail(), MAX_MAILBOX);
        assert_eq!(
            events.lock().unwrap().len(),
            MAX_MAILBOX,
            "the dropped message emitted a false delivered/queued event"
        );
        assert!(live
            .take_mail()
            .iter()
            .all(|mail| mail.content != "dropped-33"));
    }

    #[tokio::test]
    async fn retired_child_mail_is_rejected_and_never_persisted_by_a_successor_run() {
        use crate::tests_support::{HarnessBuilder, Script};

        let mut h = HarnessBuilder::new(vec![
            Script::text("first answer"),
            Script::text("successor answer"),
        ])
        .build()
        .await;
        let inner = Arc::clone(h.runtime.shared());
        let first = h.submit("first run").await;
        let first_events = h.collect_until_finished().await;
        assert!(first_events.iter().any(|event| matches!(
            &event.event,
            Event::RunFinished { run_id, status, .. }
                if run_id == &first.run_id && status == "completed"
        )));
        assert!(h.wait_idle().await.is_empty());

        // Keep the exact tool context an old child would still hold while it
        // unwinds after the parent retired its run mailbox.
        let (old_ctx, published) = control_test_ctx_for_run(
            &inner,
            h.session(),
            &first.run_id,
            Some("a_late_cancelled_child"),
        );
        let tool = SendMessageTool {
            inner: Arc::downgrade(&inner),
        };
        let send_late = |content: &'static str| {
            tool_request(SEND_MESSAGE_TOOL, serde_json::json!({"message": content}))
        };
        let after_cleanup = tool
            .invoke(send_late("late-old canceled instruction"), old_ctx.clone())
            .await;
        assert!(!after_cleanup.ok, "{}", after_cleanup.content);
        assert_eq!(after_cleanup.meta.unwrap()["delivered"], 0);
        assert!(after_cleanup.content.contains("no longer running"));
        assert!(after_cleanup.content.contains("nothing was queued"));

        let successor = h.submit("successor run").await;
        assert_ne!(successor.run_id, first.run_id);
        // The session now has a different active generation. The same old sender
        // still cannot address it implicitly, before or after its first drain.
        let after_successor = tool
            .invoke(
                send_late("late-old instruction after successor start"),
                old_ctx,
            )
            .await;
        assert!(!after_successor.ok, "{}", after_successor.content);
        assert_eq!(after_successor.meta.unwrap()["delivered"], 0);
        let successor_events = h.collect_until_finished().await;
        assert!(successor_events.iter().any(|event| matches!(
            &event.event,
            Event::RunFinished { run_id, status, .. }
                if run_id == &successor.run_id && status == "completed"
        )));
        assert!(published.lock().expect("published").is_empty());

        let messages = h.session_messages().await;
        for forbidden in [
            "late-old canceled instruction",
            "late-old instruction after successor start",
        ] {
            assert!(
                !messages
                    .iter()
                    .any(|message| message.content.contains(forbidden)),
                "retired mail was persisted: {messages:?}"
            );
            assert!(
                !h.factory.requests().iter().any(|request| request
                    .messages
                    .iter()
                    .any(|message| message.content.contains(forbidden))),
                "retired mail reached the successor's provider: {forbidden}"
            );
        }
    }

    #[tokio::test]
    async fn a_named_settled_child_returns_its_persisted_summary_after_hub_removal() {
        let inner = crate::tests_support::test_inner();
        let mut record = status_record("a_done", AgentStatus::Completed);
        record.summary = "the real persisted child summary".into();
        record.finished_at = Some(100);
        inner.deps.store.upsert_agent(&record).await.unwrap();
        let live = inner.hub.insert(
            &record,
            record.parent_run_id.clone(),
            1,
            CancellationToken::new(),
        );
        crate::hub::AgentHub::mark_settled(&live, AgentStatus::Completed);
        inner.hub.finish(&record.id, AgentStatus::Completed);
        assert!(inner.hub.get(&record.id).is_none());

        let (ctx, _) = control_test_ctx(&inner, "s_status", None);
        let outcome = WaitForAgentsTool {
            inner: Arc::downgrade(&inner),
        }
        .invoke(
            tool_request(
                WAIT_FOR_AGENTS_TOOL,
                serde_json::json!({"agents": [record.id]}),
            ),
            ctx,
        )
        .await;
        assert!(outcome.ok, "{}", outcome.content);
        assert!(outcome.content.contains("a_done"));
        assert!(outcome.content.contains("completed"));
        assert!(outcome.content.contains("the real persisted child summary"));
        assert_eq!(outcome.meta.unwrap()["finished"][0]["status"], "completed");
    }

    #[tokio::test]
    async fn a_named_settled_wait_rejects_unknown_and_other_session_rows() {
        let inner = crate::tests_support::test_inner();
        let mut foreign = status_record("a_foreign_done", AgentStatus::Completed);
        foreign.session_id = "s_other".into();
        foreign.summary = "must never cross the session boundary".into();
        inner.deps.store.upsert_agent(&foreign).await.unwrap();
        let (ctx, _) = control_test_ctx(&inner, "s_status", None);
        let tool = WaitForAgentsTool {
            inner: Arc::downgrade(&inner),
        };
        for id in ["a_missing", "a_foreign_done"] {
            let outcome = tool
                .invoke(
                    tool_request(
                        WAIT_FOR_AGENTS_TOOL,
                        serde_json::json!({"agents": [id], "timeout_secs": 1}),
                    ),
                    ctx.clone(),
                )
                .await;
            assert!(!outcome.ok, "waiting for {id} unexpectedly succeeded");
            assert!(outcome.content.contains("no agent"), "{}", outcome.content);
            assert!(!outcome.content.contains("must never cross"));
        }
    }

    #[tokio::test]
    async fn a_named_settled_wait_enforces_persisted_ownership() {
        let inner = crate::tests_support::test_inner();
        for (id, parent, summary) in [
            ("a_owner", None, "owner"),
            ("a_owned_done", Some("a_owner"), "owned terminal summary"),
            ("a_unowned_done", None, "unowned terminal summary"),
        ] {
            let mut record = status_record(id, AgentStatus::Completed);
            record.parent_agent_id = parent.map(str::to_string);
            record.summary = summary.into();
            inner.deps.store.upsert_agent(&record).await.unwrap();
        }
        let (ctx, _) = control_test_ctx(&inner, "s_status", Some("a_owner"));
        let tool = WaitForAgentsTool {
            inner: Arc::downgrade(&inner),
        };
        let owned = tool
            .invoke(
                tool_request(
                    WAIT_FOR_AGENTS_TOOL,
                    serde_json::json!({"agents": ["a_owned_done"]}),
                ),
                ctx.clone(),
            )
            .await;
        assert!(owned.ok, "{}", owned.content);
        assert!(owned.content.contains("owned terminal summary"));

        let unowned = tool
            .invoke(
                tool_request(
                    WAIT_FOR_AGENTS_TOOL,
                    serde_json::json!({"agents": ["a_unowned_done"]}),
                ),
                ctx,
            )
            .await;
        assert!(!unowned.ok);
        assert!(unowned.content.contains("not yours to control"));
        assert!(!unowned.content.contains("unowned terminal summary"));
    }

    #[tokio::test]
    async fn opposing_http_and_model_controls_keep_live_and_persisted_states_equal() {
        use crate::tests_support::{HarnessBuilder, Script};

        let h = HarnessBuilder::new(vec![Script::text("unused")])
            .build()
            .await;
        let inner = Arc::clone(h.runtime.shared());
        let mut record = status_record("a_shared_control", AgentStatus::Running);
        record.session_id = h.session().into();
        h.store.upsert_agent(&record).await.unwrap();
        let live = inner.hub.insert(
            &record,
            record.parent_run_id.clone(),
            1,
            CancellationToken::new(),
        );
        let (model_ctx, _) = control_test_ctx(&inner, h.session(), None);

        // HTTP wins the guard and parks its store write. The model resume must not
        // set live=false and then CAS running before the paused write has landed.
        let pause_gate = h.store.gate_next("transition_agent_status");
        let pausing = {
            let runtime = Arc::clone(&h.runtime);
            tokio::spawn(async move {
                runtime
                    .control_agent("a_shared_control", crate::AgentAction::Pause)
                    .await
            })
        };
        pause_gate.wait_entered().await;
        assert!(live.is_paused());
        let resuming = {
            let inner = Arc::downgrade(&inner);
            let ctx = model_ctx.clone();
            tokio::spawn(async move {
                PauseAgentsTool {
                    inner,
                    resume: true,
                }
                .invoke(
                    tool_request(
                        RESUME_AGENTS_TOOL,
                        serde_json::json!({"agents": ["a_shared_control"]}),
                    ),
                    ctx,
                )
                .await
            })
        };
        tokio::task::yield_now().await;
        pause_gate.release();
        assert_eq!(pausing.await.unwrap().unwrap(), AgentStatus::Paused);
        assert!(resuming.await.unwrap().ok);
        assert!(!live.is_paused());
        assert_eq!(
            inner
                .deps
                .store
                .get_agent(&record.id)
                .await
                .unwrap()
                .unwrap()
                .agent
                .status,
            "running"
        );

        // Reverse the callers and directions: model pause enters first and HTTP
        // resume waits behind its store write.
        let pause_gate = h.store.gate_next("transition_agent_status");
        let pausing = {
            let inner = Arc::downgrade(&inner);
            let ctx = model_ctx.clone();
            tokio::spawn(async move {
                PauseAgentsTool {
                    inner,
                    resume: false,
                }
                .invoke(
                    tool_request(
                        PAUSE_AGENTS_TOOL,
                        serde_json::json!({"agents": ["a_shared_control"]}),
                    ),
                    ctx,
                )
                .await
            })
        };
        pause_gate.wait_entered().await;
        assert!(live.is_paused());
        let http_resume = {
            let runtime = Arc::clone(&h.runtime);
            tokio::spawn(async move {
                runtime
                    .control_agent("a_shared_control", crate::AgentAction::Resume)
                    .await
            })
        };
        tokio::task::yield_now().await;
        pause_gate.release();
        assert!(pausing.await.unwrap().ok);
        assert_eq!(http_resume.await.unwrap().unwrap(), AgentStatus::Running);
        assert!(!live.is_paused());
        assert_eq!(
            inner
                .deps
                .store
                .get_agent(&record.id)
                .await
                .unwrap()
                .unwrap()
                .agent
                .status,
            "running"
        );

        // Start from paused, let HTTP resume enter first, then oppose it with a
        // model pause. The final state must be paused in memory and in SQLite.
        let pause = inner.hub.control(&record.id).await.unwrap();
        pause.pause("test").unwrap();
        mark_paused(&inner, pause.live(), true, AgentStatus::Paused)
            .await
            .unwrap();
        drop(pause);
        let resume_gate = h.store.gate_next("transition_agent_status");
        let resuming = {
            let runtime = Arc::clone(&h.runtime);
            tokio::spawn(async move {
                runtime
                    .control_agent("a_shared_control", crate::AgentAction::Resume)
                    .await
            })
        };
        resume_gate.wait_entered().await;
        assert!(!live.is_paused());
        let pausing = {
            let inner = Arc::downgrade(&inner);
            let ctx = model_ctx.clone();
            tokio::spawn(async move {
                PauseAgentsTool {
                    inner,
                    resume: false,
                }
                .invoke(
                    tool_request(
                        PAUSE_AGENTS_TOOL,
                        serde_json::json!({"agents": ["a_shared_control"]}),
                    ),
                    ctx,
                )
                .await
            })
        };
        tokio::task::yield_now().await;
        resume_gate.release();
        assert_eq!(resuming.await.unwrap().unwrap(), AgentStatus::Running);
        assert!(pausing.await.unwrap().ok);
        assert!(live.is_paused());
        assert_eq!(
            inner
                .deps
                .store
                .get_agent(&record.id)
                .await
                .unwrap()
                .unwrap()
                .agent
                .status,
            "paused"
        );
    }

    #[tokio::test]
    async fn report_status_fault_races_preserve_pause_resume_and_terminal_fields() {
        use crate::tests_support::{HarnessBuilder, Script};

        let h = HarnessBuilder::new(vec![Script::text("unused")])
            .build()
            .await;
        let inner = Arc::clone(h.runtime.shared());
        let mut record = status_record("a_note_race", AgentStatus::Running);
        record.session_id = h.session().into();
        h.store.upsert_agent(&record).await.unwrap();
        inner.hub.insert(
            &record,
            record.parent_run_id.clone(),
            1,
            CancellationToken::new(),
        );
        let (ctx, _) = control_test_ctx(&inner, h.session(), Some("a_note_race"));

        for (resume, expected, note, progress) in [
            (false, AgentStatus::Paused, "pause race", Some(0.4)),
            (true, AgentStatus::Running, "resume race", Some(0.8)),
        ] {
            let gate = h.store.gate_next("update_agent_note");
            let reporting = {
                let inner = Arc::downgrade(&inner);
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    ReportStatusTool { inner }
                        .invoke(
                            tool_request(
                                REPORT_STATUS_TOOL,
                                serde_json::json!({"note": note, "progress": progress}),
                            ),
                            ctx,
                        )
                        .await
                })
            };
            gate.wait_entered().await;
            let control = inner.hub.control("a_note_race").await.unwrap();
            let status = if resume {
                control.resume("test").unwrap()
            } else {
                control.pause("test").unwrap()
            };
            assert_eq!(
                mark_paused(&inner, control.live(), !resume, status)
                    .await
                    .unwrap(),
                expected
            );
            drop(control);
            gate.release();
            assert!(reporting.await.unwrap().ok);

            let after = inner
                .deps
                .store
                .get_agent("a_note_race")
                .await
                .unwrap()
                .unwrap()
                .agent;
            assert_eq!(after.status, expected.as_str());
            assert_eq!(after.note, note);
            assert_eq!(after.progress, progress);
            assert_eq!(after.summary, record.summary);
            assert_eq!(after.iterations, record.iterations);
            assert_eq!(after.finished_at, record.finished_at);
        }

        // The same narrow write also cannot replay stale iterations/summary or a
        // live status over a terminal full-row finalization.
        let gate = h.store.gate_next("update_agent_note");
        let reporting = {
            let inner = Arc::downgrade(&inner);
            let ctx = ctx.clone();
            tokio::spawn(async move {
                ReportStatusTool { inner }
                    .invoke(
                        tool_request(
                            REPORT_STATUS_TOOL,
                            serde_json::json!({"note": "terminal race", "progress": 1.0}),
                        ),
                        ctx,
                    )
                    .await
            })
        };
        gate.wait_entered().await;
        let mut terminal = h
            .store
            .get_agent("a_note_race")
            .await
            .unwrap()
            .unwrap()
            .agent;
        terminal.status = AgentStatus::Completed.as_str().into();
        terminal.iterations = 7;
        terminal.summary = "terminal summary".into();
        terminal.finished_at = Some(9_999);
        h.store.upsert_agent(&terminal).await.unwrap();
        gate.release();
        assert!(reporting.await.unwrap().ok);
        let after = h
            .store
            .get_agent("a_note_race")
            .await
            .unwrap()
            .unwrap()
            .agent;
        assert_eq!(after.status, "completed");
        assert_eq!(after.iterations, 7);
        assert_eq!(after.summary, "terminal summary");
        assert_eq!(after.finished_at, Some(9_999));
        assert_eq!(after.note, "terminal race");
        assert_eq!(after.progress, Some(1.0));
    }

    #[tokio::test]
    async fn cancellation_during_delayed_task_append_finalizes_claims_without_a_loop() {
        use crate::tests_support::{HarnessBuilder, Script};

        let h = HarnessBuilder::new(vec![Script::text("unused")])
            .build()
            .await;
        let inner = Arc::clone(h.runtime.shared());
        let (mut ctx, _) = control_test_ctx(&inner, h.session(), None);
        let cancel = CancellationToken::new();
        ctx.cancel = cancel.clone();
        let gate = h.store.gate_next("append_message");
        let spawned = {
            let inner = Arc::clone(&inner);
            let task_cancel = cancel.clone();
            tokio::spawn(async move {
                let spawn = SpawnRequest {
                    task: "cancel while preparing".into(),
                    name: Some("delayed".into()),
                    role: "worker".into(),
                    owned_paths: vec!["delayed/file.txt".into()],
                    model: None,
                };
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => None,
                    outcome = spawn_batch(
                        &inner,
                        &ctx,
                        vec![spawn],
                        &SpawnOptions { background: false, wait: false, worktree: None },
                        false,
                    ) => Some(outcome),
                }
            })
        };

        gate.wait_entered().await;
        let agents = h.store.agents();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].status, "running");
        assert_eq!(h.store.claims().len(), 1);
        assert!(
            inner.hub.get(&agents[0].id).is_none(),
            "preparation has no live supervisor yet"
        );

        cancel.cancel();
        assert!(
            spawned.await.unwrap().is_none(),
            "the cancelled spawn future must be dropped"
        );
        gate.release();
        let agent_id = agents[0].id.clone();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let agent = h.store.get_agent(&agent_id).await.unwrap().unwrap().agent;
            if agent.status == "cancelled" && h.store.claims().is_empty() {
                assert!(agent.finished_at.is_some());
                assert!(agent
                    .error
                    .unwrap_or_default()
                    .contains("before the subagent supervisor"));
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "unsupervised row/claims were not cleaned up"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(inner.hub.list_all().is_empty());
        assert!(h.wait_idle().await.is_empty());
        assert_eq!(
            inner.subagents.active(),
            0,
            "the unspawned child's slot must be released"
        );
    }

    #[tokio::test]
    async fn pause_and_resume_never_overwrite_a_terminal_record() {
        let inner = crate::tests_support::test_inner();
        for (id, paused, hub_status) in [
            ("a_pause_loses", true, AgentStatus::Paused),
            ("a_resume_loses", false, AgentStatus::Running),
        ] {
            let mut record = status_record(id, AgentStatus::Running);
            inner.deps.store.upsert_agent(&record).await.unwrap();
            let live = inner.hub.insert(
                &record,
                record.parent_run_id.clone(),
                1,
                CancellationToken::new(),
            );

            // This is the final full-row write that used to race the control
            // read/upsert. Its result and metadata must all remain authoritative.
            record.status = AgentStatus::Completed.as_str().into();
            record.iterations = 7;
            record.summary = "terminal result".into();
            record.updated_at = 999;
            record.finished_at = Some(1_000);
            record.note = "finished".into();
            record.progress = Some(1.0);
            inner.deps.store.upsert_agent(&record).await.unwrap();

            let actual = mark_paused(&inner, &live, paused, hub_status)
                .await
                .expect("terminal CAS miss is an observed state, not a store error");
            assert_eq!(actual, AgentStatus::Completed);
            let after = inner.deps.store.get_agent(id).await.unwrap().unwrap().agent;
            assert_eq!(after.status, "completed");
            assert_eq!(after.iterations, 7);
            assert_eq!(after.summary, "terminal result");
            assert_eq!(after.updated_at, 999);
            assert_eq!(after.finished_at, Some(1_000));
            assert_eq!(after.note, "finished");
            assert_eq!(after.progress, Some(1.0));
            inner.hub.finish(id, AgentStatus::Completed);
        }
    }

    #[tokio::test]
    async fn pause_and_resume_transition_only_live_states() {
        let inner = crate::tests_support::test_inner();
        let record = status_record("a_control", AgentStatus::Running);
        inner.deps.store.upsert_agent(&record).await.unwrap();
        let live = inner.hub.insert(
            &record,
            record.parent_run_id.clone(),
            1,
            CancellationToken::new(),
        );
        assert_eq!(
            mark_paused(&inner, &live, true, AgentStatus::Paused)
                .await
                .unwrap(),
            AgentStatus::Paused
        );
        assert_eq!(
            mark_paused(&inner, &live, false, AgentStatus::Running)
                .await
                .unwrap(),
            AgentStatus::Running
        );
        let after = inner
            .deps
            .store
            .get_agent(&record.id)
            .await
            .unwrap()
            .unwrap()
            .agent;
        assert_eq!(after.status, "running");
        assert_eq!(after.created_at, 10);
        assert!(after.updated_at >= 10);
    }

    fn subagent_config_for_test(max_concurrent: usize) -> kc_core::config::SubagentConfig {
        kc_core::config::SubagentConfig {
            max_concurrent,
            max_iterations: 6,
            allow_recursive: true,
            max_depth: 3,
            timeout_secs: 10,
            allow_background: true,
            max_background: 4,
            wait_timeout_secs: 5,
            worktrees: false,
        }
    }

    #[test]
    fn spec_is_a_delegate_tool_with_required_task() {
        struct Dummy;
        #[async_trait]
        impl Tool for Dummy {
            fn spec(&self) -> ToolSpec {
                spawn_spec(false)
            }
            async fn invoke(&self, _r: ToolCallRequest, _c: ToolCtx) -> ToolOutcome {
                ToolOutcome::ok("")
            }
        }
        let spec = Dummy.spec();
        assert_eq!(spec.name, SPAWN_AGENT_TOOL);
        assert_eq!(Dummy.class(), ToolClass::ReadOnly); // default class, not the spec's
        assert_eq!(spec.origin, kc_core::tool::ToolOrigin::Builtin);
        assert_eq!(spec.parameters["required"], serde_json::json!(["task"]));
        assert!(spec.description.contains("subagent"));
        // The background switch has to be advertised, or no model will use it.
        assert!(spec.parameters["properties"]["background"].is_object());
    }

    #[test]
    fn the_fan_out_spec_takes_a_batch() {
        let spec = spawn_spec(true);
        assert_eq!(spec.name, SPAWN_AGENTS_TOOL);
        assert_eq!(spec.parameters["required"], serde_json::json!(["tasks"]));
        assert!(spec.description.contains("at the same time"));
        assert!(spec.parameters["properties"]["tasks"]["items"]["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|v| v == "task")));
    }

    #[test]
    fn a_task_entry_is_parsed_or_refused() {
        let ok = parse_task(&serde_json::json!({
            "task": "  review the diff  ",
            "name": "reviewer",
            "owned_paths": ["src/lib.rs"]
        }))
        .expect("valid task");
        assert_eq!(ok.task, "review the diff");
        assert_eq!(ok.name.as_deref(), Some("reviewer"));
        assert_eq!(ok.role, "worker");
        assert_eq!(ok.owned_paths, vec!["src/lib.rs".to_string()]);

        assert!(parse_task(&serde_json::json!({ "task": "   " })).is_err());
        assert!(parse_task(&serde_json::json!("nope")).is_err());
        assert!(parse_task(&serde_json::json!({ "task": "x", "owned_paths": "nope" })).is_err());
    }

    #[test]
    fn background_flags_must_be_booleans() {
        let request = |value: serde_json::Value| ToolCallRequest {
            call_id: "c".into(),
            name: SPAWN_AGENT_TOOL.into(),
            arguments: value,
        };
        let options = parse_spawn_options(&request(serde_json::json!({ "task": "x" }))).unwrap();
        assert!(!options.background && !options.wait);
        let options =
            parse_spawn_options(&request(serde_json::json!({ "background": true }))).unwrap();
        assert!(options.background);
        assert!(parse_spawn_options(&request(serde_json::json!({ "background": "yes" }))).is_err());
    }

    /// `worktree` is a tri-state, not a flag: absent and `null` both mean "whoever
    /// is above me decides", so a client that always sends the key cannot pin
    /// every spawn by accident.
    #[test]
    fn the_worktree_argument_is_a_tri_state() {
        let request = |value: serde_json::Value| ToolCallRequest {
            call_id: "c".into(),
            name: SPAWN_AGENT_TOOL.into(),
            arguments: value,
        };
        for absent in [
            serde_json::json!({ "task": "x" }),
            serde_json::json!({ "task": "x", "worktree": null }),
        ] {
            let options = parse_spawn_options(&request(absent)).unwrap();
            assert_eq!(options.worktree, None);
        }
        let on = parse_spawn_options(&request(serde_json::json!({ "worktree": true }))).unwrap();
        assert_eq!(on.worktree, Some(true));
        let off = parse_spawn_options(&request(serde_json::json!({ "worktree": false }))).unwrap();
        assert_eq!(off.worktree, Some(false));
        assert!(
            parse_spawn_options(&request(serde_json::json!({ "worktree": "yes" }))).is_err(),
            "a non-boolean must be refused, not coerced"
        );
    }

    /// The three scopes, in the order the docs promise: the call, then the
    /// session, then the install.
    #[test]
    fn the_narrowest_worktree_scope_wins() {
        let plan = |on: bool, source: WorktreeSource| WorktreePlan { on, source };
        assert_eq!(
            resolve_worktrees(Some(true), Some(false), false),
            plan(true, WorktreeSource::Call),
            "a per-call argument beats both other scopes, in either direction"
        );
        assert_eq!(
            resolve_worktrees(Some(false), None, true),
            plan(false, WorktreeSource::Call)
        );
        assert_eq!(
            resolve_worktrees(None, Some(true), false),
            plan(true, WorktreeSource::Session)
        );
        assert_eq!(
            resolve_worktrees(None, Some(false), true),
            plan(false, WorktreeSource::Session),
            "a session that turned it off stays off after the install default is turned on"
        );
        assert_eq!(
            resolve_worktrees(None, None, true),
            plan(true, WorktreeSource::Global)
        );
        assert_eq!(
            resolve_worktrees(None, None, false),
            plan(false, WorktreeSource::Global)
        );
    }

    /// A refusal has to point at the switch the caller can actually change: the
    /// install-wide message alone is useless advice to somebody who asked for one
    /// round of isolated children.
    #[test]
    fn a_refusal_names_the_scope_that_asked_for_isolation() {
        assert!(WorktreeSource::Global.refusal_note().is_none());
        let call = WorktreeSource::Call.refusal_note().unwrap();
        assert!(call.contains("worktree: true"), "{call}");
        let session = WorktreeSource::Session.refusal_note().unwrap();
        assert!(session.contains("subagent_worktrees"), "{session}");
    }

    #[test]
    fn an_unknown_model_override_is_refused_with_a_usable_alternative() {
        let known = vec!["gateway/model-a".to_string(), "gateway/model-b".to_string()];
        // The shapes actually seen in the database: a bare family name, and the
        // name of the agent being spawned.
        for bad in ["sonnet", "gpt-5", "database-audit", "default"] {
            let reason = unknown_model_message(&known, bad);
            assert!(reason.contains(&format!("`{bad}`")), "{reason}");
            assert!(reason.contains("gateway/model-a"), "{reason}");
            assert!(reason.contains("Omit `model`"), "{reason}");
        }
    }

    #[test]
    fn a_long_catalog_is_summarised_instead_of_dumped() {
        let known: Vec<String> = (0..40).map(|i| format!("gateway/model-{i}")).collect();
        let reason = unknown_model_message(&known, "sonnet");
        assert!(reason.contains("40 model(s)"), "{reason}");
        assert!(reason.contains("model-7"), "{reason}");
        assert!(
            !reason.contains("model-8,"),
            "only the first few are listed: {reason}"
        );
        assert!(reason.contains("(and 32 more)"), "{reason}");
    }
}
