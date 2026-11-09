/**
 * The browser half of a passkey ceremony: `navigator.credentials.create` and
 * `.get`, plus the base64url conversions the wire format needs.
 *
 * The server never sees a private key and the client never sees a challenge it
 * can reuse: the browser signs `authenticatorData || SHA-256(clientDataJSON)`
 * with the key the authenticator holds, and posts the three fields back. This
 * module's only job is to hand WebAuthn the right `ArrayBuffer`s and to turn its
 * answer back into the JSON the server verifies.
 *
 * Everything is feature-detected: a browser without WebAuthn (or a page served
 * over plain `http://` on a non-loopback host, where it is unavailable) gets a
 * message instead of a button that does nothing.
 */

import type {
  PasskeyAssertionResult,
  PasskeyRegistrationOptions,
  PasskeyRegistrationResult,
  PasskeyRequestOptions,
} from '../api/types';

/** Why a ceremony could not be completed, in words a person can act on. */
export class PasskeyError extends Error {
  readonly cancelled: boolean;

  constructor(message: string, cancelled = false) {
    super(message);
    this.name = 'PasskeyError';
    this.cancelled = cancelled;
  }
}

/** Whether this browser can run a passkey ceremony at all. */
export function passkeySupported(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof navigator !== 'undefined' &&
    typeof navigator.credentials !== 'undefined' &&
    typeof window.PublicKeyCredential !== 'undefined'
  );
}

/**
 * Whether a platform authenticator (Touch ID, Windows Hello, Android's screen
 * lock) is available. Best-effort: `false` only means "not a platform one",
 * which is why a security key still works.
 */
export async function platformAuthenticatorAvailable(): Promise<boolean> {
  if (!passkeySupported()) return false;
  try {
    return await window.PublicKeyCredential!.isUserVerifyingPlatformAuthenticatorAvailable();
  } catch {
    return false;
  }
}

/** base64url (with or without padding) to bytes. */
export function base64UrlToBytes(value: string): Uint8Array {
  const normalized = value.replace(/-/g, '+').replace(/_/g, '/');
  const padded = normalized + '='.repeat((4 - (normalized.length % 4)) % 4);
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

/** Bytes to base64url, no padding - the spelling the server expects. */
export function bytesToBase64Url(bytes: ArrayBuffer | Uint8Array): string {
  const view = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let binary = '';
  for (let index = 0; index < view.length; index += 1) {
    binary += String.fromCharCode(view[index] ?? 0);
  }
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function bufferOf(bytes: Uint8Array): ArrayBuffer {
  // A `Uint8Array` may be a view into a larger buffer; WebAuthn needs exactly
  // these bytes.
  return bytes.slice().buffer;
}

function describeCredentialError(error: unknown): PasskeyError {
  if (error instanceof PasskeyError) return error;
  const name = (error as { name?: string } | null)?.name ?? '';
  switch (name) {
    case 'NotAllowedError':
      return new PasskeyError(
        'The passkey prompt was dismissed or timed out. Try again, and approve the prompt on your device.',
        true,
      );
    case 'SecurityError':
      return new PasskeyError(
        'This page is not allowed to use passkeys. A passkey has to be created and used on the same site over HTTPS (or on localhost).',
      );
    case 'InvalidStateError':
      return new PasskeyError(
        'This device already has a passkey for this server. Sign in with it, or remove it from the device first.',
      );
    case 'NotSupportedError':
      return new PasskeyError(
        'This browser or device does not support the kind of passkey this server requires (ES256).',
      );
    case 'AbortError':
      return new PasskeyError('The passkey request was cancelled.', true);
    default: {
      const message = (error as { message?: string } | null)?.message;
      return new PasskeyError(message ?? 'The passkey ceremony failed.');
    }
  }
}

/** Register a new passkey on this device. */
export async function createPasskey(
  options: PasskeyRegistrationOptions,
): Promise<PasskeyRegistrationResult> {
  if (!passkeySupported()) {
    throw new PasskeyError(
      'This browser has no WebAuthn support, so a passkey cannot be created here. Use a password instead.',
    );
  }
  let credential: Credential | null;
  try {
    credential = await navigator.credentials.create({
      publicKey: {
        challenge: bufferOf(base64UrlToBytes(options.challenge)),
        rp: { id: options.rp.id, name: options.rp.name },
        user: {
          id: bufferOf(base64UrlToBytes(options.user.id)),
          name: options.user.name,
          displayName: options.user.displayName,
        },
        pubKeyCredParams: [
          { type: 'public-key', alg: -7 },
          { type: 'public-key', alg: -257 },
        ],
        timeout: options.timeout_ms,
        // `none` keeps the ceremony local: this server does not make attestation
        // trust decisions, so asking for attestation would add a consent prompt
        // for nothing.
        attestation: 'none',
        authenticatorSelection: {
          residentKey: 'preferred',
          userVerification: 'preferred',
        },
        excludeCredentials: (options.exclude ?? []).map((entry) => ({
          type: 'public-key' as const,
          id: bufferOf(base64UrlToBytes(entry.id)),
        })),
      },
    });
  } catch (error) {
    throw describeCredentialError(error);
  }
  const response = (credential as PublicKeyCredential | null)?.response as
    | AuthenticatorAttestationResponse
    | undefined;
  if (!credential || !response) {
    throw new PasskeyError('The authenticator returned nothing to register.');
  }
  return {
    // `credential.id` is base64url text per spec; keep it verbatim so the id the
    // server stores is the id the browser will send next time.
    credential_id: credential.id,
    client_data_json: bytesToBase64Url(response.clientDataJSON),
    attestation_object: bytesToBase64Url(response.attestationObject),
  };
}

/** Sign in with an existing passkey. */
export async function getPasskeyAssertion(
  options: PasskeyRequestOptions,
): Promise<PasskeyAssertionResult> {
  if (!passkeySupported()) {
    throw new PasskeyError(
      'This browser has no WebAuthn support, so a passkey cannot be used here. Sign in with the password instead.',
    );
  }
  let credential: Credential | null;
  try {
    credential = await navigator.credentials.get({
      publicKey: {
        challenge: bufferOf(base64UrlToBytes(options.challenge)),
        rpId: options.rp_id,
        timeout: options.timeout_ms,
        userVerification: 'preferred',
        allowCredentials: (options.allow ?? []).map((entry) => ({
          type: 'public-key' as const,
          id: bufferOf(base64UrlToBytes(entry.id)),
        })),
      },
    });
  } catch (error) {
    throw describeCredentialError(error);
  }
  const response = (credential as PublicKeyCredential | null)?.response as
    | AuthenticatorAssertionResponse
    | undefined;
  if (!credential || !response) {
    throw new PasskeyError('The authenticator returned no assertion.');
  }
  return {
    credential_id: credential.id,
    client_data_json: bytesToBase64Url(response.clientDataJSON),
    authenticator_data: bytesToBase64Url(response.authenticatorData),
    signature: bytesToBase64Url(response.signature),
  };
}
