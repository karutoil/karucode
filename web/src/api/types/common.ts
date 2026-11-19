// Common API primitives mirroring docs/API.md.

export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

export type ErrorCode =
  | 'bad_request'
  | 'not_found'
  | 'permission_denied'
  | 'unauthorized'
  | 'config'
  | 'provider'
  | 'tool'
  | 'store'
  | 'plugin'
  | 'timeout'
  | 'path_escape'
  | 'payload_too_large'
  | 'update'
  | 'internal';

export interface ApiErrorBody {
  error: { code: ErrorCode; message: string };
}
