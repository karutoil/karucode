import type { JSX } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { describeApprovals } from '../lib/approvalCopy';
import { useFocusTrap } from '../lib/dom';
import * as actions from '../state/actions';
import { runStatusLabel } from '../state/selectors';
import * as store from '../state/store';
import type { InspectorTab } from '../state/store';
import * as workbench from '../state/workbench';
import { SessionInfoTab } from './SessionInfoTab';
import { TabPanel, Tabs, type TabItem } from './Tabs';
import { AlertIcon, ArrowRightIcon, ChatIcon, CloseIcon, LayersIcon, SettingsIcon } from './icons';

const SECTION_ITEMS: readonly TabItem<workbench.ContextSection>[] = [
  { id: 'now', label: 'Now' },
  { id: 'usage', label: 'Usage' },
  { id: 'access', label: 'Access' },
];

/** Close Context, then reveal a real result through the existing tab action. */
function revealResult(tab: InspectorTab): void {
  workbench.closeContext();
  actions.selectInspectorTab(tab);
  if (store.narrowViewport.value) {
    store.mobileSheet.value = 'inspector';
  } else if (store.inspectorCollapsed.value) {
    store.inspectorCollapsed.value = false;
    store.persistBool('inspector', false);
  }
}

/** A conversation link is a deliberate navigation away from a phone sheet. */
function showConversation(): void {
  workbench.closeContext();
  if (store.narrowViewport.value) store.mobileSheet.value = null;
}

function Fact({
  label,
  value,
  title,
}: {
  label: string;
  value: JSX.Element | string;
  title?: string;
}): JSX.Element {
  return (
    <div class="context-fact">
      <dt>{label}</dt>
      <dd title={title}>{value}</dd>
    </div>
  );
}

function ResultLink({
  tab,
  children,
  onRevealResult,
}: {
  tab: InspectorTab;
  children: string;
  onRevealResult: (tab: InspectorTab) => void;
}): JSX.Element {
  return (
    <button type="button" class="context-result-link" onClick={() => onRevealResult(tab)}>
      <span>{children}</span>
      <ArrowRightIcon size={13} />
    </button>
  );
}

/**
 * Always visible above the selected Context section. Approvals and failures stay
 * discoverable without opening Now, while their real controls remain in chat.
 */
function RunRail({ onShowConversation }: { onShowConversation: () => void }): JSX.Element {
  const chat = store.chat.value;
  const activity = store.activity.value;
  const pending = store.pendingApprovalList.value;
  const runState = chat.running
    ? 'Run active'
    : chat.runStatus
      ? `Last run: ${runStatusLabel(chat.runStatus)}`
      : 'No active run';
  const runError = chat.runError?.message ?? null;
  const error = runError ?? store.chatError.value;

  return (
    <section class="context-run-rail" aria-label="Current run and approvals">
      <div class="context-run-state">
        <span class={`status-dot status-${activity.kind}`} aria-hidden="true" />
        <span>
          <strong>{activity.label}</strong>
          <span class="context-run-detail">{runState}</span>
        </span>
      </div>
      <div class="context-run-links">
        {pending.length > 0 ? (
          <a href="#chat-main" class="context-conversation-link is-pending" onClick={onShowConversation}>
            {pending.length} pending approval{pending.length === 1 ? '' : 's'}
          </a>
        ) : null}
        {error ? (
          <a href="#chat-main" class="context-conversation-link is-error" onClick={onShowConversation} title={error}>
            <AlertIcon size={12} /> {runError ? 'Run error' : 'Session error'}
          </a>
        ) : null}
        <a href="#chat-main" class="context-conversation-link" onClick={onShowConversation}>
          <ChatIcon size={12} /> Conversation
        </a>
      </div>
    </section>
  );
}

function NowSection({
  onRevealResult,
  onShowConversation,
}: {
  onRevealResult: (tab: InspectorTab) => void;
  onShowConversation: () => void;
}): JSX.Element {
  const project = store.currentProject.value;
  const session = store.activeSession.value;
  const config = store.config.value;
  const selection = store.modelSelection.value;
  const sessionWorkspace = session?.workspace.trim() || null;
  const toolWorkspace =
    sessionWorkspace ?? project?.path ?? store.health.value?.workspace ?? config?.workspace ?? null;
  const plugins = store.pluginPanelTabs.value;

  return (
    <div class="context-section">
      <section class="context-card" aria-labelledby="context-workspace-title">
        <div class="context-section-heading">
          <LayersIcon size={14} />
          <h3 id="context-workspace-title">Workspace and session</h3>
        </div>
        <dl class="context-facts">
          <Fact
            label="Project"
            value={project?.name ?? 'No project selected'}
            title={project?.path}
          />
          <Fact label="Project path" value={project?.path ?? 'Not available'} title={project?.path} />
          <Fact
            label="Session"
            value={session ? session.title || 'Untitled session' : store.currentSessionId.value ?? 'No session open'}
            title={session?.id ?? store.currentSessionId.value ?? undefined}
          />
          <Fact
            label="Session workspace"
            value={sessionWorkspace ?? (session ? 'Not reported' : 'No session workspace')}
            title={sessionWorkspace ?? undefined}
          />
          <Fact label="Activity" value={store.activity.value.label} />
          <Fact label="Connection" value={store.connectionLabel.value} />
          <Fact
            label="Pending approvals"
            value={`${store.pendingApprovalList.value.length}`}
          />
        </dl>
        {session || store.currentSessionId.value ? (
          <a href="#chat-main" class="context-session-link" onClick={onShowConversation}>
            <ChatIcon size={13} /> Open this session in the conversation
          </a>
        ) : null}
      </section>

      <section class="context-card" aria-labelledby="context-model-title">
        <div class="context-section-heading">
          <h3 id="context-model-title">Model for the next run</h3>
          <span class="context-badge">{selection.pinned ? 'Session pin' : 'Install default'}</span>
        </div>
        <dl class="context-facts">
          <Fact label="Model" value={selection.modelId ?? 'Not configured'} />
          <Fact label="Provider" value={selection.providerId ?? 'Not configured'} />
          <Fact label="Reasoning" value={selection.effort ?? 'Endpoint default'} />
        </dl>
        <p class="context-note">
          {selection.pinned
            ? 'This session carries its own provider and model pin.'
            : 'This session follows the install-wide provider and model selection.'}
        </p>
      </section>

      <section class="context-card" aria-labelledby="context-input-title">
        <h3 id="context-input-title">What the agent can use</h3>
        <dl class="context-facts">
          <Fact label="Tool-reachable workspace" value={toolWorkspace ?? 'No workspace reported'} title={toolWorkspace ?? undefined} />
        </dl>
        <p class="context-note">
          Tools can reach that workspace subject to the install’s approval policy. Opening a panel
          does not attach its contents or a UI snapshot to the next prompt.
        </p>
        <p class="context-note">
          Only the composed message and the attachments or explicit @ paths added in Composer are
          sent as message input. Use Composer to add those deliberately.
        </p>
      </section>

      <section class="context-card" aria-labelledby="context-results-title">
        <h3 id="context-results-title">Results</h3>
        <div class="context-result-links" role="group" aria-label="Open result panels">
          <ResultLink tab="workspace" onRevealResult={onRevealResult}>Files</ResultLink>
          <ResultLink tab="changes" onRevealResult={onRevealResult}>Source control</ResultLink>
          <ResultLink tab="session" onRevealResult={onRevealResult}>Session usage</ResultLink>
          <ResultLink tab="agents" onRevealResult={onRevealResult}>Agents</ResultLink>
          {plugins.map((panel) => (
            <ResultLink tab={panel.hosted_id} key={panel.hosted_id} onRevealResult={onRevealResult}>
              {panel.title}
            </ResultLink>
          ))}
        </div>
        <p class="context-note">
          These results read their real sources on demand. Terminal tabs are server-global, not a
          per-session activity feed or an automatic prompt attachment.
        </p>
      </section>
    </div>
  );
}

function UsageSection(): JSX.Element {
  return (
    <div class="context-usage">
      <p class="context-note context-usage-note">
        Context fill is the newest provider-reported main-agent prompt over the declared model
        window. Session token totals are cumulative across calls. Unreported metrics stay unknown,
        never a fabricated zero.
      </p>
      <SessionInfoTab />
    </div>
  );
}

function AccessSection(): JSX.Element {
  const project = store.currentProject.value;
  const session = store.activeSession.value;
  const approval = describeApprovals(store.config.value);

  return (
    <div class="context-section">
      <section class="context-card" aria-labelledby="context-access-title">
        <div class="context-section-heading">
          <h3 id="context-access-title">Current access policy</h3>
          {approval ? <span class="context-badge">Approvals: {approval.label}</span> : null}
        </div>
        {approval ? (
          <p class="context-policy" title={approval.title}>{approval.title}</p>
        ) : (
          <p class="context-policy">Approval policy has not been read from this install.</p>
        )}
        <button
          type="button"
          class="button button-small context-settings-link"
          onClick={() => {
            workbench.closeContext();
            actions.openSettings();
          }}
        >
          <SettingsIcon size={13} /> Open approval settings
        </button>
        <p class="context-note">
          This policy is install-scoped, not a per-message switch. Command allowlists and
          auto-approved paths remain owned by Settings → Approvals.
        </p>
      </section>

      <section class="context-card" aria-labelledby="context-access-scope-title">
        <h3 id="context-access-scope-title">Scope in view</h3>
        <dl class="context-facts">
          <Fact label="Project" value={project ? `${project.name} — ${project.path}` : 'No project selected'} />
          <Fact label="Session" value={session ? session.title || 'Untitled session' : 'No session open'} />
          <Fact label="Session workspace" value={session?.workspace || 'No session workspace'} />
        </dl>
      </section>
    </div>
  );
}

export interface ContextDrawerProps {
  /**
   * ChatView's mounted workspace supplies this to restore its local `view='chat'`
   * mode, close Context and reveal the real tab/sheet without unmounting the
   * other full-width panes. The drawer never writes that local mode itself.
   */
  onRevealResult?: (tab: InspectorTab) => void;
  /** The same caller-owned return to conversation mode for existing chat links. */
  onShowConversation?: () => void;
}

/**
 * Context is a sibling overlay, never an alternative Inspector render. Its only
 * lifetime is the transient workbench signal; results stay mounted underneath.
 * Standalone callers retain the legacy helpers as defaults; ChatView injects the
 * callbacks above so full Git/Terminal return through its workspace mode.
 */
export function ContextDrawer({
  onRevealResult = revealResult,
  onShowConversation = showConversation,
}: ContextDrawerProps = {}): JSX.Element | null {
  const open = workbench.contextOpen.value;
  const section = workbench.contextSection.value;
  const narrow = store.narrowViewport.value;
  const drawerRef = useRef<HTMLDivElement>(null);
  useFocusTrap(drawerRef, open && narrow, workbench.closeContext);

  // A resize into the phone layout while Context is open still gives the drawer
  // the existing inspector sheet contract without remounting either surface.
  useEffect(() => {
    if (open && narrow && store.mobileSheet.value !== 'inspector') workbench.openContext(section);
  }, [open, narrow]);

  useEffect(() => () => {
    // A removed workspace must not leave contextOpen suppressing a future phone
    // Inspector. Closing here also restores a snapshot borrowed on that phone.
    if (workbench.contextOpen.value) workbench.closeContext();
  }, []);

  if (!open) return null;

  return (
    <div
      ref={drawerRef}
      id="workbench-context"
      class="context-drawer is-open"
      role="dialog"
      aria-modal={narrow ? 'true' : undefined}
      aria-label="Workspace context"
    >
      <header class="context-drawer-head">
        <div>
          <span class="context-kicker">Workbench</span>
          <h2>Context</h2>
        </div>
        <button
          type="button"
          class="icon-button context-close"
          onClick={workbench.closeContext}
          aria-label="Close context"
          data-autofocus={narrow ? true : undefined}
        >
          <CloseIcon />
        </button>
      </header>
      <Tabs
        items={SECTION_ITEMS}
        active={section}
        onChange={workbench.selectContextSection}
        label="Context sections"
        idPrefix="context"
      />
      <RunRail onShowConversation={onShowConversation} />
      <TabPanel idPrefix="context" id="now" active={section === 'now'}>
        <NowSection onRevealResult={onRevealResult} onShowConversation={onShowConversation} />
      </TabPanel>
      <TabPanel idPrefix="context" id="usage" active={section === 'usage'}>
        <UsageSection />
      </TabPanel>
      <TabPanel idPrefix="context" id="access" active={section === 'access'}>
        <AccessSection />
      </TabPanel>
    </div>
  );
}
