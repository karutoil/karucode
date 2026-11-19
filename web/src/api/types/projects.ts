/** A workspace the agent may work in; one entry in the project rail. */
export interface Project {
  id: string;
  name: string;
  /** Canonical absolute directory. */
  path: string;
  created_at: number;
  updated_at: number;
  session_count: number;
  /** Workbench result-pane width; absent means the product default. */
  result_panel_width?: number;
}

export interface ProjectListResponse {
  projects: Project[];
  /**
   * The server's implicit workspace (KARUCODE_WORKSPACE / --workspace), or `null`
   * when it was started without one — then the project list is the only source of
   * directories.
   */
  workspace: string | null;
}
