export interface ConfigMcpSummary {
  name: string;
  transport: string;
  status: string;
  tools: string[];
  error: string | null;
}

export interface McpServer {
  name: string;
  transport: string;
  status: string;
  tools: string[];
  error: string | null;
}

/** What one `POST /api/mcp/reload` changed, in configuration order. */
export interface McpRescanReport {
  added: string[];
  removed: string[];
  restarted: string[];
  retried: string[];
  unchanged: string[];
}

export interface McpRescanResponse {
  report: McpRescanReport;
  servers: McpServer[];
}
