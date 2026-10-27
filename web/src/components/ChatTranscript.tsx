import type { JSX } from 'preact';
import type { ApprovalView, ChatItem } from '../state/chat';
import { ApprovalCard } from './ApprovalCard';
import { CommandCard } from './CommandCard';
import { Markdown } from './Markdown';
import { ThinkingBlock } from './ThinkingBlock';
import { ToolCard } from './ToolCard';

export interface ChatTranscriptProps {
  items: ChatItem[];
  resolvingApprovals: Record<string, boolean>;
  onApprovalDecision: (approvalId: string, decision: 'allow_once' | 'allow_session' | 'deny') => void;
}

export function ChatTranscript({
  items,
  resolvingApprovals,
  onApprovalDecision,
}: ChatTranscriptProps): JSX.Element {
  return (
    <div class="transcript">
      {items.map((item) => {
        if (item.kind === 'message') {
          if (item.role === 'user') {
            return (
              <article class="turn turn-user" key={item.id} aria-label="Your message">
                <div class="turn-label">You</div>
                <div class="user-text">{item.content}</div>
              </article>
            );
          }
          return (
            <article
              class={`turn turn-assistant${item.streaming ? ' is-streaming' : ''}`}
              key={item.id}
              aria-label="karucode response"
            >
              <div class="turn-label">
                karucode
                {item.streaming ? <span class="stream-label-live">streaming</span> : null}
              </div>
              {item.reasoning ? (
                <ThinkingBlock reasoning={item.reasoning} streaming={item.streaming && item.content === ''} />
              ) : null}
              {item.content ? <Markdown text={item.content} /> : null}
              {item.streaming && item.content === '' && item.reasoning === '' ? (
                <p class="thinking-placeholder" aria-hidden="true">
                  Thinking
                  <span class="thinking-dots">
                    <span>.</span>
                    <span>.</span>
                    <span>.</span>
                  </span>
                </p>
              ) : null}
            </article>
          );
        }
        if (item.kind === 'tool') return <ToolCard key={item.id} item={item} />;
        if (item.kind === 'command') {
          return <CommandCard key={item.id} item={item} defaultOpen={item.status === 'running'} />;
        }
        if (item.kind === 'approval') {
          const approval: ApprovalView = item.approval;
          return (
            <ApprovalCard
              key={item.id}
              approval={approval}
              resolving={Boolean(resolvingApprovals[approval.id])}
              onDecide={(decision) => onApprovalDecision(approval.id, decision)}
            />
          );
        }
        if (item.kind === 'interrupted') {
          return (
            <div class="notice notice-interrupted" role="status" key={item.id}>
              <strong>Interrupted.</strong> {item.message} Send a new message to continue.
            </div>
          );
        }
        if (item.kind === 'error') {
          return (
            <div class="run-error" role="alert" key={item.id}>
              <p class="run-error-title">Error</p>
              <p>{item.message}</p>
            </div>
          );
        }
        if (item.kind === 'notice') {
          return (
            <p
              class={`notice notice-${item.level}`}
              role={item.level === 'error' ? 'alert' : 'status'}
              key={item.id}
            >
              {item.message}
            </p>
          );
        }
        return null;
      })}
    </div>
  );
}