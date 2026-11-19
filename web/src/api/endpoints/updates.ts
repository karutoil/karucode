import { apiRequest } from '../client';
import type { UpdateApplyResponse, UpdateStatus } from '../types/updates';
import type { CallOptions } from './common';

// ---------------------------------------------------------------------------
// Versioning and updates
// ---------------------------------------------------------------------------

/** `GET /api/update` — cached; never touches the release host. */
export function getUpdate(options: CallOptions = {}): Promise<UpdateStatus> {
  return apiRequest<UpdateStatus>('/api/update', options);
}

/**
 * `POST /api/update/check` — refresh from the release manifest now.
 *
 * A failed check answers `409`/`502` **and** records the reason in the status, so
 * the caller can show both the error and the panel it belongs to.
 */
export function checkForUpdate(options: CallOptions = {}): Promise<UpdateStatus> {
  return apiRequest<UpdateStatus>('/api/update/check', {
    ...options,
    method: 'POST',
    body: {},
  });
}

/**
 * `POST /api/update/apply` — stage if needed, then install.
 *
 * Needs the admin credential when one is configured (`X-Karucode-Admin-Token`);
 * that header is not stored in the browser, so a locked read-only install answers
 * `403` and the panel says to run the printed command instead.
 */
export function applyUpdate(
  request: { restart?: boolean } = {},
  options: CallOptions = {},
): Promise<UpdateApplyResponse> {
  return apiRequest<UpdateApplyResponse>('/api/update/apply', {
    ...options,
    method: 'POST',
    body: request,
  });
}
