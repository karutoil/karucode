import type { ApiErrorBody, ErrorCode } from './types';

const TOKEN_KEY = 'karucode.token';

function readStoredToken(): string | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

let authToken: string | null = readStoredToken();
let unauthorizedHandler: (() => void) | null = null;

export function getAuthToken(): string | null {
  return authToken;
}

export function setAuthToken(token: string | null): void {
  authToken = token && token.trim() !== '' ? token.trim() : null;
  try {
    if (typeof localStorage !== 'undefined') {
      if (authToken) localStorage.setItem(TOKEN_KEY, authToken);
      else localStorage.removeItem(TOKEN_KEY);
    }
  } catch {
    // Private mode / storage disabled: the in-memory token still works.
  }
}

/** Called by the app to switch to the token entry screen on a 401. */
export function onUnauthorized(handler: (() => void) | null): void {
  unauthorizedHandler = handler;
}

export class ApiError extends Error {
  readonly status: number;
  readonly code: ErrorCode | 'network' | 'unknown';

  constructor(message: string, status: number, code: ErrorCode | 'network' | 'unknown') {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }

  get isNetwork(): boolean {
    return this.code === 'network';
  }

  get isUnauthorized(): boolean {
    return this.status === 401 || this.code === 'unauthorized';
  }
}

export interface RequestOptions {
  method?: 'GET' | 'POST' | 'PATCH' | 'PUT' | 'DELETE';
  body?: unknown;
  signal?: AbortSignal;
  /** Override the stored token (used by the auth screen's connection test). */
  token?: string | null;
  fetchImpl?: typeof fetch;
  headers?: Record<string, string>;
}

function isErrorBody(value: unknown): value is ApiErrorBody {
  if (typeof value !== 'object' || value === null) return false;
  const candidate = (value as { error?: unknown }).error;
  return (
    typeof candidate === 'object' &&
    candidate !== null &&
    typeof (candidate as { code?: unknown }).code === 'string' &&
    typeof (candidate as { message?: unknown }).message === 'string'
  );
}

/**
 * JSON fetch wrapper for the documented API. Adds the bearer token when one is
 * stored and the CSRF header required on mutations. Throws ApiError for both
 * HTTP failures and network failures so callers only handle one error type.
 */
export async function apiRequest<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const method = options.method ?? 'GET';
  const headers: Record<string, string> = {
    Accept: 'application/json',
    ...options.headers,
  };
  if (method !== 'GET') headers['X-Karucode-Client'] = 'web';
  const token = options.token !== undefined ? options.token : authToken;
  if (token) headers.Authorization = `Bearer ${token}`;

  let body: string | undefined;
  if (options.body !== undefined) {
    headers['Content-Type'] = 'application/json';
    body = JSON.stringify(options.body);
  }

  const fetchImpl = options.fetchImpl ?? globalThis.fetch;
  let response: Response;
  try {
    response = await fetchImpl(path, {
      method,
      headers,
      body,
      signal: options.signal,
      credentials: 'same-origin',
    });
  } catch (error) {
    if (error instanceof DOMException && error.name === 'AbortError') throw error;
    throw new ApiError(
      error instanceof Error ? `Could not reach karucode: ${error.message}` : 'Could not reach karucode',
      0,
      'network',
    );
  }

  const text = await response.text();
  let parsed: unknown = undefined;
  if (text.trim() !== '') {
    try {
      parsed = JSON.parse(text) as unknown;
    } catch {
      throw new ApiError(
        `Unexpected non-JSON response from ${path} (HTTP ${response.status})`,
        response.status,
        'unknown',
      );
    }
  }

  if (!response.ok) {
    if (response.status === 401) unauthorizedHandler?.();
    if (isErrorBody(parsed)) {
      throw new ApiError(parsed.error.message, response.status, parsed.error.code);
    }
    throw new ApiError(`Request failed with HTTP ${response.status}`, response.status, 'unknown');
  }

  return parsed as T;
}

export function toErrorMessage(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  if (error instanceof Error) return error.message;
  return String(error);
}

/** Query string helper; omits undefined/null values. */
export function withQuery(path: string, query: Record<string, string | number | boolean | null | undefined>): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value === undefined || value === null) continue;
    params.set(key, String(value));
  }
  const qs = params.toString();
  return qs ? `${path}?${qs}` : path;
}