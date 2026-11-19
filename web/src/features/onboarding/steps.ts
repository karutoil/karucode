import type { ApprovalMode, TunnelKind } from '../../api/types';

/**
 * The wizard's data: what the six steps are, and the choices each one offers.
 *
 * Kept out of the component so the rail, the step bodies and the summary all
 * read the same list — a seventh step is one entry here plus one body, and the
 * "Step n of m" counter cannot disagree with what is on screen.
 */

export type StepId = 'welcome' | 'provider' | 'appearance' | 'approvals' | 'access' | 'done';

export interface StepSpec {
  id: StepId;
  title: string;
  blurb: string;
}

export const STEPS: readonly StepSpec[] = [
  {
    id: 'welcome',
    title: 'Welcome to karucode',
    blurb: 'Four decisions in six short steps: the model, the look, the approvals and how this server is reached.',
  },
  {
    id: 'provider',
    title: 'Connect a model provider',
    blurb: 'karucode talks to any OpenAI-compatible endpoint. Nothing works until this does.',
  },
  {
    id: 'appearance',
    title: 'Make it yours',
    blurb: 'Theme, density and how tool calls are drawn. Stored in this browser.',
  },
  {
    id: 'approvals',
    title: 'Decide what the agent may do alone',
    blurb: 'This is the human gate. It applies to every project and every session.',
  },
  {
    id: 'access',
    title: 'Who can reach this server',
    blurb:
      'A password or passkey, and — if you reach karucode through a tunnel — the name it answers to.',
  },
  {
    id: 'done',
    title: 'Ready',
    blurb: 'Everything you chose is saved. You can change any of it later in Settings.',
  },
];

export const APPROVAL_CHOICES: readonly { value: ApprovalMode; label: string; description: string }[] = [
  {
    value: 'ask',
    label: 'Ask me (recommended)',
    description: 'Shell commands and file changes wait for your approval.',
  },
  {
    value: 'ask_commands',
    label: 'Ask for shell commands only',
    description: 'File writes inside the project are allowed; commands still ask.',
  },
  {
    value: 'read_only',
    label: 'Read only',
    description: 'The agent may look, never change. Useful for a first cautious session.',
  },
  {
    value: 'auto',
    label: 'Never ask',
    description: 'Everything is allowed and recorded. Only for a disposable machine.',
  },
];

export const TUNNELS: readonly { value: TunnelKind; label: string; description: string }[] = [
  {
    value: 'none',
    label: 'No tunnel',
    description: 'Reached directly on this machine or your own network.',
  },
  {
    value: 'cloudflare',
    label: 'Cloudflare Tunnel',
    description:
      'cloudflared forwards from this machine. Cloudflare’s published proxy ranges are trusted automatically.',
  },
  {
    value: 'custom',
    label: 'Another reverse proxy',
    description: 'nginx, Caddy, Traefik, Tailscale — name the addresses it forwards from.',
  },
];

export const DEFAULT_BASE_URL = 'https://api.openai.com/v1';

/** The access step's draft: everything it collects before one save. */
export interface AccessDraft {
  enabled: boolean;
  domain: string;
  tunnel: TunnelKind;
  proxies: string;
  password: string;
  confirm: string;
}

/** The result of the provider step's "Test connection", already worded. */
export interface ProviderTestResult {
  ok: boolean;
  message: string;
  models: string[];
}

/** A readable default label for a passkey, from what the browser will admit to. */
export function defaultPasskeyName(): string {
  try {
    const agent = navigator.userAgent;
    if (/iPhone|iPad/.test(agent)) return 'iPhone or iPad';
    if (/Macintosh/.test(agent)) return 'Mac (Touch ID)';
    if (/Windows/.test(agent)) return 'Windows Hello';
    if (/Android/.test(agent)) return 'Android passkey';
    if (/Linux/.test(agent)) return 'This device';
  } catch {
    // No navigator (tests, or a hardened browser): the server has a default too.
  }
  return 'Passkey';
}
