import type { JSX } from 'preact';
import type { AgentSummary, ControlChatSummary, Project, SessionSummary } from '../../api/types';
import { isAgentLive } from '../../api/types';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import { copyText } from '../../lib/clipboard';
import { absoluteTime, relativeTime, truncate } from '../../lib/format';
import { agentsForControlChat } from '../../state/controlPlane';
import { agentStatusLabel } from '../../state/selectors';
import { ChatView } from '../ChatView';
import {
  AgentsIcon,
  ChevronDownIcon,
  ChevronLeftIcon,
  ExternalIcon,
  SparkIcon,
} from '../icons';
import { chatTitle, controlStarter, sessionStatus } from './controlModel';
import '../../styles/control-session.css';

export interface ControlSessionProps {
  /** The open session's id. */
  selected: string;
  /** The row this id names, from either list the rail draws. */
  chosen: SessionSummary | ControlChatSummary | null;
  /** True when the open conversation is a control chat rather than a project session. */
  control: boolean;
  /** A navigation is in flight. */
  opening: boolean;
  /** `store.currentSessionId.value === selected`: the workspace has really opened it. */
  ready: boolean;
  /** The session's own agent read; the snapshot's live agents answer meanwhile. */
  detailAgents: AgentSummary[] | null;
  /** The agent count the chat's own row claims, so the inspector cannot under-report. */
  claimedTotal: number | null;
  /** Registered projects, for the intro's "start in" buttons and the facts' path. */
  projects: Project[];
  /** A session's project name, or "No project". */
  projectName: (id: string | null | undefined) => string;
  onBack: () => void;
  onOpenInProject: () => void;
  onNavigate: (session: { id: string; project_id: string | null }) => void;
}

/**
 * The control chat's first-run state: its job is starting sessions, not exploring code.
 *
 * The frame is the workbench's own empty state (`.empty-state` / `.empty-symbol` /
 * `.empty-title` / `.empty-copy`, `chat.css`), because this is the same role the
 * starter prompts play in an empty project conversation. What stays the plane's is
 * the row of "start in" buttons: a control chat has projects to name, not files.
 */
function ControlChatIntro({ projects, onStart }: {
  projects: Project[];
  onStart: (project: { id: string; name: string }) => void;
}): JSX.Element {
  return <div class="empty-state">
    <div class="empty-content">
      <span class="empty-symbol" aria-hidden="true"><SparkIcon size={24} /></span>
      <h2 class="empty-title">What should we build?</h2>
      <p class="empty-copy">
        Mention a project with <strong>@</strong>, or just name it. The control chat resolves it, opens a
        normal session in that project’s workspace with its own tools and approval policy, and the new
        session shows up here.
      </p>
      <div class="cp-intro-projects">
        <span class="eyebrow">Start in</span>
        {projects.length === 0
          ? <span class="cp-intro-empty">Add a project first.</span>
          : projects.map((project) => (
              <button
                type="button"
                class="button button-small"
                key={project.id}
                title={project.name}
                onClick={() => onStart(project)}
              >
                {/* `.button` is nowrap with no max-width, so a long project name
                    ran the button off a 390px screen. The name yields and
                    ellipsises; the tooltip keeps the full spelling. */}
                <span class="cp-intro-project-name">{project.name}</span>
              </button>
            ))}
      </div>
      <p class="cp-intro-hint">You can also ask for a status summary, or discuss the plan before anything starts.</p>
    </div>
  </div>;
}

/**
 * Facts beside an open conversation, in the inspector's own vocabulary.
 *
 * This is the panel the project workspace draws in its **Session** tab, so it is
 * built from the same pieces: an `.inspector-section` with an `.inspector-kicker`
 * head, `.metric` tiles for the two numbers, and a `.si-meta` list for the labelled
 * rows (`inspector.css`). It is not a second facts component with its own type
 * scale - that is what makes the same session look like two products.
 *
 * Two facts are deliberately absent because another element already owns them:
 * the project (named in the header's breadcrumb, and its path is here) and the
 * session's state (the header's `.session-state` word, which is also what the phone
 * tier keeps readable). One owner per fact, or the two copies drift apart.
 */
function SessionFacts({ session, projects }: {
  session: SessionSummary | null;
  projects: Project[];
}): JSX.Element {
  const project = session?.project_id
    ? projects.find((item) => item.id === session.project_id) ?? null
    : null;
  const copyId = (): void => {
    if (!session) return;
    void copyText(session.id).then((ok) =>
      store.pushToast(ok ? 'Session id copied.' : 'Could not copy the session id.', ok ? 'info' : 'error'));
  };
  return <section class="inspector-section si-info" aria-label="Session details">
    <div class="inspector-kicker-row">
      <span class="inspector-kicker">Session</span>
    </div>

    <div class="si-tiles">
      <div class="metric">
        <div class="metric-num" title="Messages in this session’s transcript">
          {session ? session.message_count.toLocaleString() : '—'}
        </div>
        <div class="metric-label">Messages</div>
      </div>
      <div class="metric">
        <div class="metric-num" title={session ? absoluteTime(session.updated_at) : undefined}>
          {session ? relativeTime(session.updated_at) : '—'}
        </div>
        <div class="metric-label">Last activity</div>
      </div>
    </div>

    <dl class="si-meta">
      {project ? <div><dt>Path</dt><dd title={project.path}>{project.path}</dd></div> : null}
      <div>
        <dt>Session id</dt>
        <dd class="cp-facts-id">
          <span title={session?.id ?? undefined}>{session?.id ?? '—'}</span>
          <button type="button" class="link-button" onClick={copyId} disabled={!session}>Copy</button>
        </dd>
      </div>
    </dl>

    {session?.running
      ? <div class="cp-facts-actions">
          <button type="button" class="button button-small button-danger-ghost" onClick={() => void actions.stopRun()}>
            Stop this run
          </button>
        </div>
      : null}
  </section>;
}

/**
 * A subagent's claimed progress, when it made one.
 *
 * The same markup as `AgentsTab.tsx`'s `ProgressBar`, which is file-private; both
 * draw `.agent-progress`, so the two cannot look different. Returning nothing for
 * a missing or non-finite claim is what keeps `width: 'NaN%'` off the card - the
 * server omits `progress` rather than sending `null`.
 */
function AgentProgress({ value, label }: { value: number | null | undefined; label: string }): JSX.Element | null {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null;
  const pct = Math.round(Math.max(0, Math.min(1, value)) * 100);
  return <span
    class="agent-progress"
    role="progressbar"
    aria-label={label}
    aria-valuemin={0}
    aria-valuemax={100}
    aria-valuenow={pct}
    aria-valuetext={`${pct}% complete`}
    title={`${pct}% complete, according to the agent`}
  >
    <span class="agent-progress-fill" style={{ width: `${pct}%` }} />
  </span>;
}

/**
 * The subagents behind the open conversation.
 *
 * An agent belongs to whichever session ran it, so a control chat that delegated
 * work lists its children here — the transcript itself only shows the spawn call.
 * `agents` is the session's own read (`GET /api/agents?session=`), so a finished
 * child stays on the list; while that read has not landed the snapshot's live
 * agents (filtered to this session) answer instead, which is why the badge in the
 * rail and this list can never tell two different stories.
 *
 * The rows are the workbench's agent card (`AgentsTab.tsx`), including its
 * `data-status`: the running/paused/failed paint comes from there, so a subagent
 * looks the same in the plane and in the project it belongs to.
 */
function Subagents({ sessionId, control, agents: read, claimedTotal }: {
  sessionId: string;
  control: boolean;
  agents: AgentSummary[] | null;
  claimedTotal: number | null;
}): JSX.Element {
  const agents = read ?? agentsForControlChat(sessionId);
  const total = Math.max(claimedTotal ?? 0, agents.length);
  const live = agents.filter((agent) => isAgentLive(agent.status)).length;
  const noun = control ? 'chat' : 'session';
  return <section class="inspector-section cp-subagents" aria-label="Subagents of this chat">
    <div class="inspector-kicker-row">
      <span class="inspector-kicker">Subagents</span>
      {total > 0
        ? <span class="inspector-kicker-note">
            {total} total{live > 0 ? ` · ${live} live` : ''}
          </span>
        : null}
    </div>
    {agents.length === 0
      ? <div class="empty-inspector">
          <p class="empty-inspector-title">{total === 0 ? 'No subagents yet' : 'No subagents loaded yet'}</p>
          <p>{total === 0
            ? `Nothing has been delegated in this ${noun}: when it calls spawn_agent, the child appears here with its status and its latest note.`
            : `This ${noun} has ${total} subagent${total === 1 ? '' : 's'}; their rows have not been read yet.`}</p>
        </div>
      : <ul class="cp-agent-list">{agents.map((agent) => <AgentRow agent={agent} key={agent.id} />)}</ul>}
  </section>;
}

/** One subagent: who it is, what it says it is doing, and its own detail. */
function AgentRow({ agent }: { agent: AgentSummary }): JSX.Element {
  const open = store.selectedAgentId.value === agent.id;
  const name = agent.name || 'subagent';
  return <li class="agent-card" data-status={agent.status}>
    <header class="agent-head">
      <div class="agent-title-row">
        <span class="agent-icon" aria-hidden="true"><AgentsIcon size={15} /></span>
        <div class="agent-titles">
          <h3 class="agent-name" title={name}>{name}</h3>
          <p class="agent-role" title={agent.role || 'agent'}>{agent.role || 'agent'}</p>
        </div>
        <span class="agent-live" title={`Subagent status: ${agentStatusLabel(agent.status)}`}>
          {agentStatusLabel(agent.status)}
        </span>
      </div>
    </header>

    {agent.task ? <p class="agent-task" title={agent.task}>{truncate(agent.task, 180)}</p> : null}
    {agent.note ? <p class="agent-note" title={agent.note}>{agent.note}</p> : null}
    <AgentProgress value={agent.progress} label={`Progress for ${name}`} />

    <div class="agent-card-foot">
      <span>{agent.iterations} iteration{agent.iterations === 1 ? '' : 's'}</span>
      <button
        type="button"
        class="agent-card-open"
        aria-expanded={open}
        aria-label={`${open ? 'Close' : 'Open'} details for ${name}`}
        title={open ? 'Close this subagent' : 'Open this subagent’s transcript'}
        onClick={() => { if (open) actions.closeAgentDetail(); else void actions.openAgentDetail(agent.id); }}
      >
        <span class="agent-card-open-label">{open ? 'Close' : 'Details'}</span>
        <ChevronDownIcon size={12} />
      </button>
    </div>

    {open ? <AgentPeek agent={agent} /> : null}
  </li>;
}

/** The newest messages of one subagent, read by `actions.openAgentDetail`. */
function AgentPeek({ agent }: { agent: AgentSummary }): JSX.Element {
  const history = store.agentHistory.value;
  if (!history || history.id !== agent.id) {
    return <p class="inspector-note">Reading {agent.name || 'this subagent'}’s transcript…</p>;
  }
  if (history.error) return <p class="inspector-note" role="status">{history.error}</p>;
  const recent = history.messages.slice(-5);
  return <div class="agent-detail">
    {recent.length === 0
      ? <p class="inspector-note">It has not written a message yet.</p>
      : recent.map((message) => <div class="agent-bubble agent-bubble-answer" key={message.id}>
          <span class="agent-bubble-role">{message.role}</span>
          <div class="agent-bubble-body">{truncate(message.content, 700)}</div>
        </div>)}
    <p class="inspector-note">{history.truncated ? 'Showing its newest messages.' : 'Its whole transcript.'}</p>
  </div>;
}

/**
 * One open conversation, with the facts and the subagents beside it.
 *
 * The chat is the real `ChatView` in its control-plane mode, so a control chat and
 * a project session are the same thing once open: the composer, the approvals and
 * the tools are the session's own.
 *
 * **One owner for the title.** This header is the page header of an open session:
 * `.main-head` / `.breadcrumb` / `.main-title`, the workbench's own header
 * (`layout.css`, overridden for the workbench in `workbench.css`), so the title,
 * the project and the state sit where they sit in a project conversation. The
 * `ChatView` inside renders a second `<header class="main-head">` with the same
 * session title; the sheet hides it (`.cp-chat .main-head`), and the plane's
 * header is the one that survives, because it is the only one that can also say
 * how to get back to the overview and how to open the session in its project.
 * What the embedded header owned - the workspace tabs, the notification bell, the
 * palette - is the workbench's chrome, not an open chat's.
 */
export function ControlSession({
  selected,
  chosen,
  control,
  opening,
  ready,
  detailAgents,
  claimedTotal,
  projects,
  projectName,
  onBack,
  onOpenInProject,
  onNavigate,
}: ControlSessionProps): JSX.Element {
  const where = control ? 'Control plane · across projects' : projectName(chosen?.project_id ?? null);
  const project = control
    ? null
    : projects.find((item) => item.id === chosen?.project_id) ?? null;
  const status = chosen ? sessionStatus(chosen) : null;
  return <section class="cp-session" aria-label="Open session">
    <header class="main-head cp-session-head">
      <button
        type="button"
        class="icon-button"
        onClick={onBack}
        aria-label="Back to overview"
        title="Back to overview"
      ><ChevronLeftIcon size={16} /></button>
      <div class="breadcrumb cp-session-id">
        <strong title={project?.path ?? where}>{where}</strong>
        <span class="crumb-sep" aria-hidden="true">/</span>
        <h2 class="main-title" title={chosen ? chatTitle(chosen) : undefined}>
          {chosen ? chatTitle(chosen) : (control ? 'Untitled chat' : 'Session')}
        </h2>
      </div>
      <div class="cp-session-actions">
        {status
          ? <span class={`session-state session-state-${status.tone}`} title={`Latest state: ${status.label}`}>
              <span class={`status-dot status-${status.tone}`} aria-hidden="true" />{status.label}
            </span>
          : null}
        {chosen ? <span class="cp-session-count">{chosen.message_count.toLocaleString()} messages</span> : null}
        {/* The model and reasoning selectors live in the composer, once. They used
            to be mounted here too, and this pair was what appeared and disappeared
            as the snapshot refreshed. */}
        {control
          ? null
          : <button
              type="button"
              class="button button-small"
              title="Open this session in its project’s workspace"
              aria-label="Open in project"
              onClick={onOpenInProject}
            ><ExternalIcon size={12} />Open in project</button>}
      </div>
    </header>

    {opening && !chosen ? <div class="cp-loading"><span class="cp-spinner" aria-hidden="true" />Opening session…</div> : null}
    {ready
      ? <div class="cp-session-body">
          <div class="cp-chat"><ChatView
            mode="control-plane"
            emptyContent={control ? <ControlChatIntro projects={projects} onStart={(project) => actions.fillComposer(controlStarter(project))} /> : undefined}
            mentionProjects={control ? projects.map((project) => ({ id: project.id, name: project.name })) : undefined}
          /></div>
          <aside class="cp-inspector" aria-label="Chat details">
            <SessionFacts session={chosen} projects={projects} />
            <Subagents
              sessionId={selected}
              control={control}
              agents={detailAgents}
              claimedTotal={claimedTotal}
            />
          </aside>
        </div>
      : !opening ? <div class="cp-loading">
          This session is not open yet.{' '}
          <button type="button" class="button button-small" onClick={() => { if (chosen) onNavigate(chosen); }}>
            Try again
          </button>
        </div> : null}
  </section>;
}
