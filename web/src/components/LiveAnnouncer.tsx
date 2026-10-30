import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import type { ChatState, CommandItem, ToolItem } from '../state/chat';
import { pendingApprovals, runStatusLabel } from '../state/selectors';

export interface LiveAnnouncerProps {
  chat: ChatState;
}

type ToolItemLike = ToolItem | CommandItem;

/** The newest tool or command card: what "tool activity" means here. */
function lastTool(chat: ChatState): ToolItemLike | null {
  for (let index = chat.items.length - 1; index >= 0; index -= 1) {
    const item = chat.items[index];
    if (item && (item.kind === 'tool' || item.kind === 'command')) return item;
  }
  return null;
}

function toolLabel(tool: ToolItemLike): string {
  return tool.kind === 'command' ? tool.command : tool.name;
}

function toolSentence(tool: ToolItemLike): string {
  const name = toolLabel(tool);
  if (tool.status === 'running') return `Running ${name}.`;
  if (tool.status === 'error') return `${name} failed.`;
  if (tool.status === 'stopped') return `${name} stopped.`;
  return `${name} finished.`;
}

function runSentence(status: ChatState['runStatus']): string {
  if (status === null) return 'Run finished.';
  return `Run ${runStatusLabel(status).toLowerCase()}.`;
}

/**
 * Announces the shape of a live run to assistive tech.
 *
 * The transcript streams token by token, so announcing the content itself would
 * be unusable — a screen reader would restart the sentence hundreds of times.
 * This region instead speaks the milestones a listener actually needs: the run
 * starting and finishing, a tool starting or finishing, and an approval waiting.
 */
export function LiveAnnouncer({ chat }: LiveAnnouncerProps): JSX.Element {
  const [announcement, setAnnouncement] = useState('');

  const tool = lastTool(chat);
  const toolKey = tool ? `${tool.id}:${tool.status}` : null;
  const approval = pendingApprovals(chat)[0] ?? null;
  const approvalKey = approval?.id ?? null;

  const previous = useRef<{
    running: boolean;
    runStatus: ChatState['runStatus'];
    tool: string | null;
    approval: string | null;
  } | null>(null);

  useEffect(() => {
    const next = {
      running: chat.running,
      runStatus: chat.runStatus,
      tool: toolKey,
      approval: approvalKey,
    };
    const before = previous.current;
    previous.current = next;
    // The first pass only records the baseline: mounting the view is not news.
    if (!before) return;

    if (next.running && !before.running) {
      setAnnouncement('karucode is working.');
      return;
    }
    if (!next.running && before.running) {
      setAnnouncement(runSentence(next.runStatus));
      return;
    }
    if (approval && next.approval !== before.approval) {
      setAnnouncement(`Waiting for approval: ${approval.title}.`);
      return;
    }
    if (tool && next.tool !== before.tool) {
      setAnnouncement(toolSentence(tool));
    }
  }, [chat.running, chat.runStatus, toolKey, approvalKey]);

  return (
    <p class="sr-only" role="status" aria-live="polite" aria-atomic="true">
      {announcement}
    </p>
  );
}
