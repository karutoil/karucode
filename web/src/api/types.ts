// Types mirroring docs/API.md. These are the contract with kc-server; keep field
// names exactly as documented so we notice drift early.
//
// Barrel re-export of the domain modules under `./types/`.

export type * from './types/common';
export type * from './types/projects';
export type * from './types/providers';
export type * from './types/auth';
export type * from './types/skills';
export type * from './types/plugins';
export type * from './types/mcp';
export type * from './types/updates';
export type * from './types/notifications';
export type * from './types/approvals';
export type * from './types/messages';
export * from './types/agents';
export type * from './types/sessions';
export type * from './types/control';
export type * from './types/config';
export type * from './types/workspace';
export type * from './types/git';
export type * from './types/terminals';
export type * from './types/events';
