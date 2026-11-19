import { useEffect, useRef, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import { contextMenuProps, Menu, MenuItem, useMenu } from '../../lib/menu';
import { copyText } from '../../lib/clipboard';
import { isImeComposing } from '../../lib/keyboard';
import type { SessionSummary } from '../../api/types';
import { formatCount, relativeTime } from '../../lib/format';
import { PluginSlot } from '../../components/PluginSlot';
import { UI_SLOT_SESSION_ROW_BADGE } from '../../components/uiContributions';
import { sessionStatusView } from '../../state/selectors';
import * as store from '../../state/store';
import type { SessionFacts } from '../../state/sessionFacts';
import { KebabIcon } from '../../components/icons';
import { IconButton, Input } from '../../ui';
import { toneClass } from './helpers';

export interface SessionRowProps {
  session: SessionSummary;
  active: boolean;
  now: number;
  /** The facts read for this session, or `null` while they are unknown. */
  facts: SessionFacts | null;
  /** Unread notifications for this session. */
  unreadCount: number;
  /**
   * Chips a plugin asked for this row (§R4), already resolved for the project
   * being drawn. A prop rather than a store read: the fly-out shows *another*
   * project's sessions, and reading the open project's annotations there would
   * paint one project's chips onto a different project's rows.
   */
  badges?: { text: string; tone: string }[];
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => void | Promise<boolean>;
  onRequestDelete: (session: SessionSummary, trigger: HTMLElement | null) => void;
}

export function SessionRow({
  session,
  active,
  now,
  facts,
  unreadCount,
  badges = [],
  onSelect,
  onRename,
  onRequestDelete,
}: SessionRowProps): JSX.Element {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(session.title);
  const menu = useMenu();
  const inputRef = useRef<HTMLInputElement>(null);
  const skipBlur = useRef(false);
  /** Open IME composition in the rename field; see the form's `onSubmit`. */
  const composing = useRef(false);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  useEffect(() => {
    if (!editing) setDraft(session.title);
  }, [session.title, editing]);

  /** The title is selectable text, so the native menu could copy it; ours must
   *  not be a regression, and the row's own spelling is the one worth keeping. */
  const copyTitle = async (): Promise<void> => {
    const title = session.title || 'Untitled session';
    const copied = await copyText(title);
    store.pushToast(copied ? 'Session title copied.' : 'Could not copy the title.',
      copied ? 'success' : 'error');
  };

  const startRename = (): void => {
    skipBlur.current = false;
    // A composition that was still open when the field went away must not make
    // the next rename uncommittable.
    composing.current = false;
    menu.close();
    setDraft(session.title);
    setEditing(true);
  };

  const cancelRename = (): void => {
    skipBlur.current = true;
    setEditing(false);
    setDraft(session.title);
  };

  const commitRename = (): void => {
    if (skipBlur.current) {
      skipBlur.current = false;
      return;
    }
    const next = draft.trim();
    setEditing(false);
    if (next !== '' && next !== session.title) void onRename(session.id, next);
    else setDraft(session.title);
  };

  // The word and the colour both come from `sessionStatusView` (`state/selectors.ts`),
  // the same helper the control plane reads: a failed run must not be spelled one
  // way here and another way there.
  const { label: statusLabel, tone: statusTone } = sessionStatusView(session);
  // A plugin's chips for this row, if any (`before_session_list`). The server
  // already capped and sanitised them; an unknown tone falls back here too, so a
  // plugin cannot paint a colour this build does not have.
  const chips = badges;
  // The mock's row classes drive the status rail: a run in flight, a failure,
  // and a session waiting on the reader (unread news) each paint it differently.
  const rowState = [
    session.running ? 'running' : '',
    session.last_run_status === 'failed' ? 'failed' : '',
    unreadCount > 0 ? 'needs' : '',
  ]
    .filter(Boolean)
    .join(' ');
  const turns = facts ? facts.turns : null;
  // Tool names and turn/call counts stay discoverable without making every row
  // repeat the same fingerprint. The visible line is reserved for state, recency
  // and the server's message count.
  const details = [
    `${session.message_count} messages`,
    turns !== null && turns > 0 ? `${turns} ${turns === 1 ? 'turn' : 'turns'}` : null,
    facts && facts.calls !== null ? `${facts.calls} ${facts.calls === 1 ? 'tool call' : 'tool calls'}` : null,
    facts && facts.tools.length > 0 ? `Tools: ${facts.tools.join(', ')}` : null,
  ].filter((detail): detail is string => detail !== null).join(' · ');

  return (
    <li
      class={`session-row session-row-quiet${active ? ' is-active' : ''}${rowState ? ` ${rowState}` : ''}`}
      data-session-id={session.id}
      {...contextMenuProps(menu, () => !editing, () => menu.triggerRef.current)}
    >
      {editing ? (
        <form
          class="session-rename"
          onSubmit={(event) => {
            event.preventDefault();
            // A title is typed in the reader's own language, and an IME's Enter
            // both commits its candidate and implicitly submits this form - the
            // title at that instant is half a word. The `onKeyDown` below stops
            // the submit at the key; this is the second net for engines that
            // have already ended the composition by then.
            if (composing.current) return;
            commitRename();
          }}
        >
          {/* `data-escape-local`: Escape cancels the rename here; the sheet's
            * focus trap defers to this control instead of closing the sheet. */}
          <Input
            inputRef={inputRef}
            size="md"
            class="session-rename-input h-[30px] px-4 bg-surface border-accent-line max-lg:text-title"
            value={draft}
            aria-label={`Rename session ${session.title}`}
            maxLength={200}
            data-escape-local
            onCompositionStart={() => {
              composing.current = true;
            }}
            onCompositionEnd={() => {
              composing.current = false;
            }}
            onInput={(event) => setDraft(event.currentTarget.value)}
            onBlur={commitRename}
            onKeyDown={(event) => {
              // While an IME is composing, Enter is its candidate-commit key and
              // Firefox spells it `keyCode === 229` with `isComposing` false.
              // Only Enter is claimed: this is the form's only field, so an
              // unclaimed Enter is the browser's implicit submit, which would
              // rename the session to half a word. Claiming every composition
              // keydown instead can stop the IME's own text from arriving - the
              // app's other three IME guards return bare for that reason.
              if (isImeComposing(event)) {
                if (event.key === 'Enter') event.preventDefault();
                return;
              }
              if (event.key === 'Escape') {
                event.preventDefault();
                cancelRename();
              }
            }}
          />
        </form>
      ) : (
        // The row itself is not a `Button`: it carries `aria-current`, a
        // double-click to rename and two structured spans, none of which the
        // primitive's flat slot model expresses.
        <button
          type="button"
          class="session-open"
          onClick={() => onSelect(session.id)}
          onDblClick={startRename}
          aria-current={active ? 'true' : undefined}
          title={details}
        >
          <span class="session-head">
            <span class="session-title">{session.title || 'Untitled session'}</span>
            {unreadCount > 0 ? (
              <span
                class="session-unread"
                aria-label={`${unreadCount} unread notification${unreadCount === 1 ? '' : 's'}`}
                title={`${unreadCount} unread notification${unreadCount === 1 ? '' : 's'}`}
              >
                {unreadCount > 9 ? '9+' : unreadCount}
              </span>
            ) : null}
            {statusTone ? <span class={`status-dot status-${statusTone}`} aria-hidden="true" /> : null}
            {/* Chips a plugin asked for (§R4). Display only: the state lives in the
              * plugin, and a click goes through the contributed control rather
              * than anything here - this row is not a plugin's to control. */}
            {chips.map((chip) => (
              <span
                key={`${session.id}-${chip.text}`}
                class={`session-chip session-chip-${toneClass(chip.tone)}`}
              >
                {chip.text}
              </span>
            ))}
            {/* Controls a plugin contributed for this row. Mounted so the
              * `session.row.badge` slot declared by the catalogue is real; an
              * empty slot renders nothing and costs no space. */}
            <PluginSlot slot={UI_SLOT_SESSION_ROW_BADGE} />
          </span>
          <span class="session-meta">
            {session.running ? <span class="sr-only">Running. </span> : null}
            {statusLabel ? (
              <>
                <span class={`session-state session-state-${statusTone}`}>{statusLabel}</span>
                <span class="meta-dot" aria-hidden="true" />
              </>
            ) : null}
            <span>{relativeTime(session.updated_at, now)}</span>
            <span class="meta-dot" aria-hidden="true" />
            <span title={`${session.message_count} messages`}>{formatCount(session.message_count)}</span>
            {session.last_run_status === 'failed' ? <span class="sr-only">Last run failed.</span> : null}
            {session.last_run_status === 'interrupted' ? (
              <span class="sr-only">Last run interrupted.</span>
            ) : null}
          </span>
        </button>
      )}

      {!editing ? (
        <div class="session-menu-wrap">
          <IconButton
            size="sm"
            // `.icon-button` + `.session-menu-trigger` are the shared row-kebab
            // recipe (24px, revealed on row hover): keep both hooks, let the
            // primitive own the box.
            class="icon-button session-menu-trigger w-[24px] h-[24px]"
            label={`Actions for ${session.title || 'untitled session'}`}
            buttonRef={menu.triggerProps.ref}
            aria-haspopup="menu"
            expanded={menu.triggerProps['aria-expanded']}
            icon={<KebabIcon size={15} />}
            onClick={menu.triggerProps.onClick}
          />
          <Menu
            menu={menu}
            label={`Session actions for ${session.title}`}
            float={Boolean(menu.pointer)}
            class={menu.pointer ? undefined : 'session-menu'}
          >
            <MenuItem menu={menu} action={{ label: 'Rename', onSelect: startRename }} />
            <MenuItem menu={menu} action={{ label: 'Copy title', onSelect: () => void copyTitle() }} />
            <MenuItem
              menu={menu}
              action={{
                label: 'Delete…',
                danger: true,
                onSelect: () => onRequestDelete(session, menu.triggerRef.current),
              }}
            />
          </Menu>
        </div>
      ) : null}
    </li>
  );
}
