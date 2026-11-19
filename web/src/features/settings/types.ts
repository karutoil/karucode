import type { ApprovalMode, Limits, ProviderTestResult } from '../../api/types';
import type { SettingsTab } from '../../components/SettingsNav';

/**
 * Every limit the sheet edits, as a number in the form.
 *
 * Only the numeric ones: `parallel_tool_calls` is a switch and is edited as a
 * toggle, so it must not be a key of this record.
 */
export type NumericLimitKey = {
  [K in keyof Limits]: Limits[K] extends number ? K : never;
}[keyof Limits];

export interface FormState {
  /** Whether a temperature is sent at all; when off, the field is not sent. */
  sendTemperature: boolean;
  temperature: string;
  maxTokens: string;
  topP: string;
  requestTimeout: string;
  parallelToolCalls: boolean;
  /** Run a turn's independent tool calls at the same time (limits). */
  parallelExecution: boolean;
  /** Install-wide default for image input; a per-model `vision` wins. */
  supportsImages: boolean;
  approvalMode: ApprovalMode;
  allowCommands: string[];
  autoApprovePaths: string[];
  approvalTimeout: string;
  limits: Record<NumericLimitKey, string>;
  subagentConcurrent: string;
  subagentIterations: string;
  allowRecursive: boolean;
  subagentMaxDepth: string;
  subagentTimeout: string;
  allowBackground: boolean;
  subagentMaxBackground: string;
  subagentWaitTimeout: string;
  subagentWorktrees: boolean;
}

/** The collapsible "Advanced" block of each tab that has one. */
export interface AdvancedState {
  general: boolean;
  models: boolean;
  agent: boolean;
  notifications: boolean;
}

/**
 * Where each validated field lives: its tab, whether it sits behind that tab's
 * Advanced disclosure, and the DOM id to focus.
 */
export interface FieldLocation {
  tab: SettingsTab;
  advanced?: keyof AdvancedState;
  anchor: string;
}

export interface TestState {
  running: boolean;
  result: ProviderTestResult | null;
  error: string | null;
}

export interface AddProviderDraft {
  id: string;
  name: string;
  provider: string;
  baseUrl: string;
  apiKey: string;
}

export type TriState = '' | 'true' | 'false';
