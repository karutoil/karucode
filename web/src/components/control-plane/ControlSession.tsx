import type { JSX } from 'preact';
import type { AgentSummary, ControlChatSummary, Project, SessionSummary } from '../../api/types';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import { copyText } from '../../lib/clipboard';
import { agentsForControlChat } from '../../state/controlPlane';
import { ChatView } from '../ChatView';
import {
  agentStatusWord,
  chatTitle,
  controlStarter,
  initials,
  sessionStatus,
  shortMoment,
} from './controlModel';
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

/** The control chat's first-run state: its job is starting sessions, not exploring code. */
function ControlChatIntro({ projects, onStart }: {
  projects: Project[];
  onStart: (project: { id: string; name: string }) => void;
}): JSX.Element {
  return <div class="cp-intro">
    <span class="cp-intro-mark" aria-hidden="true">✳</span>
    <h2>What should we build?</h2>
    <p>
      Mention a project with <strong>@</strong>, or just name it. The control chat resolves it, opens a
      normal session in that project’s workspace with its own tools and approval policy, and the new
      session shows up here.
    </p>
    <div class="cp-intro-projects">
      <span class="cp-eyebrow">Start in</span>
      {projects.length === 0
        ? <span class="cp-intro-empty">Add a project first.</span>
        : projects.map((project) => <button type="button" key={project.id} onClick={() => onStart(project)}>{project.name}</button>)}
    </div>
    <p class="cp-intro-hint">You can also ask for a status summary, or discuss the plan before anything starts.</p>
  </div>;
}

/**
 * Facts beside an open conversation. On a wide monitor the transcript is a
 * centred column, so this panel carries the session's own numbers and the two
 * actions that would otherwise need a trip back to the project.
 */
function SessionFacts({ session, control, projects, onOpenInProject }: {
  session: SessionSummary | null;
  control: boolean;
  projects: Project[];
  onOpenInProject: () => void;
}): JSX.Element {
  const status = session ? sessionStatus(session) : null;
  const project = session?.project_id
    ? projects.find((item) => item.id === session.project_id) ?? null
    : null;
  const copyId = (): void => {
    if (!session) return;
    void copyText(session.id).then((ok) =>
      store.pushToast(ok ? 'Session id copied.' : 'Could not copy the session id.', ok ? 'info' : 'error'));
  };
  return <section class="cp-facts" aria-label="Session details">
    <div class="cp-eyebrow">Session</div>
    <dl>
      <div>
        <dt>Where</dt>
        <dd>{control ? 'Control plane' : project?.name ?? 'No project'}</dd>
      </div>
      {project ? <div><dt>Path</dt><dd class="cp-facts-path">{project.path}</dd></div> : null}
      <div>
        <dt>State</dt>
        <dd>{status ? <span class={`cp-state ${status.tone}`}><i aria-hidden="true" />{status.label}</span> : 'Unknown'}</dd>
      </div>
      <div><dt>Messages</dt><dd>{session ? session.message_count.toLocaleString() : '—'}</dd></div>
      <div><dt>Last activity</dt><dd>{session ? shortMoment(session.updated_at) : '—'}</dd></div>
      <div>
        <dt>Session id</dt>
        <dd class="cp-facts-id"><span>{session?.id ?? '—'}</span>
          <button type="button" class="cp-link" onClick={copyId} disabled={!session}>Copy</button>
        </dd>
      </div>
    </dl>
    <div class="cp-facts-actions">
      {control ? null : <button type="button" class="cp-secondary" onClick={onOpenInProject}>Open in project ↗</button>}
      {session?.running ? <button type="button" class="cp-secondary" onClick={() => void actions.stopRun()}>Stop this run</button> : null}
    </div>
  </section>;
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
 */
function Subagents({ sessionId, control, agents: read, claimedTotal }: {
  sessionId: string;
  control: boolean;
  agents: AgentSummary[] | null;
  claimedTotal: number | null;
}): JSX.Element {
  const agents = read ?? agentsForControlChat(sessionId);
  const total = Math.max(claimedTotal ?? 0, agents.length);
  const live = agents.filter((agent) => agent.status === 'running' || agent.status === 'paused').length;
  const noun = control ? 'chat' : 'session';
  return <section class="cp-subagents" aria-label="Subagents of this chat">
    <div class="cp-eyebrow">Subagents{total > 0 ? ` · ${total}` : ''}{live > 0 ? ` · ${live} live` : ''}</div>
    {agents.length === 0
      ? <p class="cp-note">{total === 0
          ? `No subagents yet — this ${noun} hasn't delegated anything.`
          : `This ${noun} has ${total} subagent${total === 1 ? '' : 's'}, none of them loaded yet.`}</p>
      : <ul class="cp-agent-list">{agents.map((agent) => <AgentRow agent={agent} key={agent.id} />)}</ul>}
  </section>;
}

/** One subagent: who it is, what it says it is doing, and its own detail. */
function AgentRow({ agent }: { agent: AgentSummary }): JSX.Element {
  const open = store.selectedAgentId.value === agent.id;
  return <li class="cp-agent">
    <div class="cp-agent-head">
      <span class={`cp-agent-dot ${agent.status === 'paused' ? 'paused' : ''}`} aria-hidden="true" />
      <span class="cp-agent-title">
        <strong>{agent.name || 'subagent'}</strong>
        <small>{agent.role || 'agent'} · {agentStatusWord(agent.status)} · {agent.iterations} iteration{agent.iterations === 1 ? '' : 's'}</small>
      </span>
      <button
        type="button"
        class="cp-link"
        aria-expanded={open}
        title={open ? 'Close this subagent' : 'Open this subagent’s transcript'}
        onClick={() => { if (open) actions.closeAgentDetail(); else void actions.openAgentDetail(agent.id); }}
      >{open ? 'Close' : 'Details'}</button>
    </div>
    {agent.note ? <p class="cp-agent-note">{agent.note}</p> : <p class="cp-agent-note muted" title={agent.task}>{agent.task}</p>}
    {open ? <AgentPeek agent={agent} /> : null}
  </li>;
}

/** The newest messages of one subagent, read by `actions.openAgentDetail`. */
function AgentPeek({ agent }: { agent: AgentSummary }): JSX.Element {
  const history = store.agentHistory.value;
  if (!history || history.id !== agent.id) {
    return <p class="cp-note">Reading {agent.name || 'this subagent'}’s transcript…</p>;
  }
  if (history.error) return <p class="cp-note" role="status">{history.error}</p>;
  const recent = history.messages.slice(-5);
  return <div class="cp-agent-detail">
    {recent.length === 0
      ? <p class="cp-note">It has not written a message yet.</p>
      : recent.map((message) => <div class="cp-agent-message" key={message.id}>
          <span class="cp-agent-role">{message.role}</span>
          <p>{message.content.length > 700 ? `${message.content.slice(0, 700)}…` : message.content}</p>
        </div>)}
    <p class="cp-note">{history.truncated ? 'Showing its newest messages.' : 'Its whole transcript.'}</p>
  </div>;
}

/**
 * One open conversation, with the facts and the subagents beside it.
 *
 * The chat is the real `ChatView` in its control-plane mode, so a control chat and
 * a project session are the same thing once open: the composer, the approvals and
 * the tools are the session's own, and only the header says where it came from.
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
  return <section class="cp-session" aria-label="Open session">
    <header class="cp-session-head">
      <button type="button" class="cp-back" onClick={onBack} aria-label="Back to overview" title="Back to overview">←</button>
      <span class={`cp-glyph${control ? ' control' : ''}`} aria-hidden="true">{control ? 'k.' : initials(projectName(chosen?.project_id ?? null))}</span>
      <div class="cp-session-id">
        <div class="cp-eyebrow">{control ? 'Control plane · across projects' : projectName(chosen?.project_id ?? null)}</div>
        <h2>{chosen ? chatTitle(chosen) : (control ? 'Untitled chat' : 'Session')}</h2>
      </div>
      <div class="cp-session-actions">
        {chosen ? <span class={`cp-state ${sessionStatus(chosen).tone}`} title={`Latest state: ${sessionStatus(chosen).label}`}><i aria-hidden="true" />{sessionStatus(chosen).label}</span> : null}
        {chosen ? <span class="cp-session-count">{chosen.message_count.toLocaleString()} messages</span> : null}
        {/* The model and reasoning selectors live in the composer, once. They
            used to be mounted here too, and this pair was what appeared and
            disappeared as the snapshot refreshed. */}
        {control ? null : <button type="button" class="cp-secondary" title="Open in project" aria-label="Open in project" onClick={onOpenInProject}>Open in project ↗</button>}
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
            <SessionFacts
              session={chosen}
              control={control}
              projects={projects}
              onOpenInProject={onOpenInProject}
            />
            <Subagents
              sessionId={selected}
              control={control}
              agents={detailAgents}
              claimedTotal={claimedTotal}
            />
          </aside>
        </div>
      : !opening ? <div class="cp-loading">This session is not open yet. <button type="button" class="cp-secondary" onClick={() => { if (chosen) onNavigate(chosen); }}>Try again</button></div> : null}
  </section>;
}
