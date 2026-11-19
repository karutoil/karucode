import { apiRequest, withQuery } from '../client';
import type { Terminal, TerminalCreateRequest, TerminalList } from '../types/terminals';
import type { CallOptions } from './common';

// ---- terminals ------------------------------------------------------------
//
// The durable half of the terminal tab: the rows that make the same tabs appear
// in every browser and after a restart. Attaching to a shell is a websocket,
// which `state/terminalSocket.ts` owns.

export function listTerminals(options: CallOptions = {}): Promise<TerminalList> {
  return apiRequest<TerminalList>('/api/terminals', options);
}

export function createTerminal(
  body: TerminalCreateRequest = {},
  options: CallOptions = {},
): Promise<{ terminal: Terminal }> {
  return apiRequest<{ terminal: Terminal }>('/api/terminals', {
    ...options,
    method: 'POST',
    body,
  });
}

/** Rename a terminal. `null` restores the derived default title. */
export function renameTerminal(
  id: string,
  title: string | null,
  options: CallOptions = {},
): Promise<{ terminal: Terminal }> {
  return apiRequest<{ terminal: Terminal }>(`/api/terminals/${encodeURIComponent(id)}`, {
    ...options,
    method: 'PATCH',
    body: { title },
  });
}

export function deleteTerminal(id: string, options: CallOptions = {}): Promise<{ deleted: string }> {
  return apiRequest<{ deleted: string }>(`/api/terminals/${encodeURIComponent(id)}`, {
    ...options,
    method: 'DELETE',
  });
}

/** Kill the shell and start a new one, keeping the id, title and position. */
export function restartTerminal(
  id: string,
  params: { cols?: number; rows?: number } = {},
  options: CallOptions = {},
): Promise<{ terminal: Terminal }> {
  return apiRequest<{ terminal: Terminal }>(
    withQuery(`/api/terminals/${encodeURIComponent(id)}/restart`, {
      cols: params.cols,
      rows: params.rows,
    }),
    { ...options, method: 'POST', body: {} },
  );
}
