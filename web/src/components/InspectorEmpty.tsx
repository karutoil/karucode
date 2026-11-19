import type { ComponentChildren, JSX } from 'preact';
import { Button, Stack, Text } from '../ui';

export interface InspectorEmptyProps {
  /** Sentence case, three or four words: what is missing, not what to do. */
  title: string;
  /** What would put something here. The reader came to answer a question. */
  children: ComponentChildren;
  /** The one action that leads out of the state, when there is one. */
  action?: { label: string; onClick: () => void; testId?: string };
}

/**
 * The empty state every inspector tab shares: a short title, one paragraph
 * explaining when the panel fills, and at most one action.
 *
 * It exists because the same three-element block was copied into the Agents,
 * Session usage and Todo panels and the copies had already drifted (two used a
 * 8px rhythm, one 12px, and only some offered a way out). The chat's own empty
 * state is a different thing — `EmptyState` is the first run's greeting — so
 * this one only ever appears inside a results panel.
 */
export function InspectorEmpty({ title, children, action }: InspectorEmptyProps): JSX.Element {
  return (
    <Stack gap="3" align="start" class="empty-inspector">
      <Text as="p" size="body" weight="semibold" class="empty-inspector-title">
        {title}
      </Text>
      <Text as="p" size="small" tone="muted">
        {children}
      </Text>
      {action ? (
        <Button variant="secondary" size="sm" testId={action.testId} onClick={action.onClick}>
          {action.label}
        </Button>
      ) : null}
    </Stack>
  );
}
