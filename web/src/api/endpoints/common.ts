export type FetchLike = typeof fetch;

export interface CallOptions {
  signal?: AbortSignal;
  fetchImpl?: FetchLike;
  /** Override the stored bearer token (used by the auth screen). */
  token?: string | null;
  /** Extra request headers (the API-key admin credential, for example). */
  headers?: Record<string, string>;
}

/**
 * The admin credential as a header, when the caller typed one.
 *
 * Key *management* needs `X-Karucode-Admin-Token` whenever
 * `KARUCODE_ADMIN_TOKEN` is configured. The browser never stores it, so the
 * Access tab asks for it the moment a management call needs it.
 */
export function adminHeaders(
  adminToken: string | undefined,
  options: CallOptions,
): Record<string, string> {
  const headers = { ...options.headers };
  const token = adminToken?.trim();
  if (token) headers['X-Karucode-Admin-Token'] = token;
  return headers;
}
