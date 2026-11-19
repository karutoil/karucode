import type { JSX } from 'preact';
import type { SessionSummary } from '../../api/types';
import type { SessionFacts } from '../../state/sessionFacts';
import { SessionRow } from './SessionRow';

export interface SessionGroupProps {
  label: string;
  items: SessionSummary[];
  currentId: string | null;
  now: number;
  facts: Record<string, SessionFacts>;
  unreadCounts: Record<string, number>;
  badgesFor: (id: string) => { text: string; tone: string }[];
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => void | Promise<boolean>;
  onRequestDelete: (session: SessionSummary, trigger: HTMLElement | null) => void;
}

export function SessionGroup({
  label,
  items,
  currentId,
  now,
  facts,
  unreadCounts,
  badgesFor,
  onSelect,
  onRename,
  onRequestDelete,
}: SessionGroupProps): JSX.Element {
  return (
    <section class="session-group" aria-label={`${label} sessions`}>
      <h2 class="session-group-label">
        {label}
        <span aria-hidden="true">{items.length}</span>
      </h2>
      <ul class="session-list">
        {items.map((session) => (
          <SessionRow
            key={session.id}
            session={session}
            active={session.id === currentId}
            now={now}
            facts={facts[session.id] ?? null}
            // The dot is the surface that works when a desktop notification
            // could not be shown (no permission, no secure context, a mobile
            // browser), and it clears when the session is opened.
            unreadCount={unreadCounts[session.id] ?? 0}
            onSelect={onSelect}
            onRename={onRename}
            badges={badgesFor(session.id)}
            onRequestDelete={onRequestDelete}
          />
        ))}
      </ul>
    </section>
  );
}
