import { apiRequest } from '../client';
import type { McpRescanResponse, McpServer } from '../types/mcp';
import type { CallOptions } from './common';

export function listMcpServers(options: CallOptions = {}): Promise<McpServer[]> {
  return apiRequest<{ servers: McpServer[] }>('/api/mcp', options).then((r) => r.servers);
}

/**
 * Re-read `[[mcp_servers]]` and apply it: new servers are started, removed ones
 * are stopped and changed ones are replaced, without restarting the server.
 *
 * The body carries the report plus the live statuses, so one call is enough to
 * update a list.
 */
export function reloadMcpServers(options: CallOptions = {}): Promise<McpRescanResponse> {
  return apiRequest<McpRescanResponse>('/api/mcp/reload', {
    ...options,
    method: 'POST',
    body: {},
  });
}

/** Replace one MCP server's process/connection (the "I rebuilt it" path). */
export function reloadMcpServer(
  name: string,
  options: CallOptions = {},
): Promise<McpServer> {
  return apiRequest<McpServer>(`/api/mcp/${encodeURIComponent(name)}/reload`, {
    ...options,
    method: 'POST',
    body: {},
  });
}
