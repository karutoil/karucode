import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  base64UrlToBytes,
  bytesToBase64Url,
  createPasskey,
  getPasskeyAssertion,
  PasskeyError,
  passkeySupported,
} from './passkey';

/* The browser half of WebAuthn is a thin adapter, and that is exactly why it is
 * worth a test: the bug it can have is a silent one. A `Uint8Array` handed
 * straight to `navigator.credentials.create` is sometimes a view into a larger
 * buffer, the challenge is base64url and the signature is DER, and getting any of
 * those wrong produces a ceremony the server refuses with no useful message.
 */

function bytes(...values: number[]): Uint8Array {
  return new Uint8Array(values);
}

describe('base64url', () => {
  it('round trips arbitrary bytes without padding', () => {
    const original = bytes(0, 1, 127, 128, 255, 254);
    const encoded = bytesToBase64Url(original);
    expect(encoded).not.toContain('=');
    expect([...base64UrlToBytes(encoded)]).toEqual([...original]);
  });

  it('accepts standard base64 and padded input', () => {
    // "aGVsbG8" is hello without padding; both spellings decode.
    expect(new TextDecoder().decode(base64UrlToBytes('aGVsbG8'))).toBe('hello');
    expect(new TextDecoder().decode(base64UrlToBytes('aGVsbG8='))).toBe('hello');
  });

  it('uses the url-safe alphabet', () => {
    // 0xfb 0xff encodes to base64 "-_8", not "+/8".
    expect(bytesToBase64Url(bytes(0xfb, 0xff))).toBe('-_8');
  });
});

describe('passkeySupported', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('is false without the WebAuthn globals', () => {
    vi.stubGlobal('PublicKeyCredential', undefined);
    expect(passkeySupported()).toBe(false);
  });
});

describe('createPasskey', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('passes the challenge, rp and user through as bytes and returns base64url', async () => {
    let seen: PublicKeyCredentialCreationOptions | undefined;
    vi.stubGlobal('PublicKeyCredential', class {});
    Object.defineProperty(navigator, 'credentials', {
      configurable: true,
      value: {
        create: vi.fn(async (options: CredentialCreationOptions) => {
          seen = options.publicKey;
          return {
            id: 'credential-1',
            type: 'public-key',
            response: {
              clientDataJSON: bytes(1, 2, 3).buffer,
              attestationObject: bytes(4, 5).buffer,
            },
          };
        }),
      },
    });

    const result = await createPasskey({
      challenge_id: 'challenge-1',
      challenge: bytesToBase64Url(bytes(9, 9, 9)),
      rp: { id: 'karucode.example.com', name: 'karucode' },
      user: { id: bytesToBase64Url(bytes(7, 7)), name: 'karucode', displayName: 'karucode' },
      timeout_ms: 60_000,
      exclude: [{ id: bytesToBase64Url(new TextEncoder().encode('existing')), type: 'public-key' }],
    });

    expect(seen?.rp).toEqual({ id: 'karucode.example.com', name: 'karucode' });
    expect([...new Uint8Array(seen!.challenge as ArrayBuffer)]).toEqual([9, 9, 9]);
    expect([...new Uint8Array(seen!.user.id as ArrayBuffer)]).toEqual([7, 7]);
    // ES256 first: the server verifies P-256 only, and a browser that silently
    // prefers RS256 would produce a credential it cannot accept.
    expect(seen?.pubKeyCredParams[0]).toEqual({ type: 'public-key', alg: -7 });
    // The exclude list is sent as bytes, so an authenticator that already holds
    // this credential refuses with InvalidStateError instead of duplicating it.
    const excluded = seen?.excludeCredentials?.[0]?.id as ArrayBuffer;
    expect(new TextDecoder().decode(new Uint8Array(excluded))).toBe('existing');
    expect(seen?.attestation).toBe('none');
    expect(result).toEqual({
      credential_id: 'credential-1',
      client_data_json: bytesToBase64Url(bytes(1, 2, 3)),
      attestation_object: bytesToBase64Url(bytes(4, 5)),
    });
  });

  it('turns a dismissed prompt into a message a person can act on', async () => {
    vi.stubGlobal('PublicKeyCredential', class {});
    Object.defineProperty(navigator, 'credentials', {
      configurable: true,
      value: {
        create: vi.fn(async () => {
          throw Object.assign(new Error('denied'), { name: 'NotAllowedError' });
        }),
      },
    });
    await expect(
      createPasskey({
        challenge_id: 'c',
        challenge: 'AAAA',
        rp: { id: 'localhost', name: 'karucode' },
        user: { id: 'AAAA', name: 'karucode', displayName: 'karucode' },
        timeout_ms: 1000,
        exclude: [],
      }),
    ).rejects.toMatchObject({ cancelled: true });
  });

  it('explains a missing WebAuthn instead of throwing a TypeError', async () => {
    vi.stubGlobal('PublicKeyCredential', undefined);
    await expect(
      createPasskey({
        challenge_id: 'c',
        challenge: 'AAAA',
        rp: { id: 'localhost', name: 'karucode' },
        user: { id: 'AAAA', name: 'karucode', displayName: 'karucode' },
        timeout_ms: 1000,
        exclude: [],
      }),
    ).rejects.toBeInstanceOf(PasskeyError);
  });
});

describe('getPasskeyAssertion', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('passes the rp id and the allow list, and returns the four wire fields', async () => {
    let seen: PublicKeyCredentialRequestOptions | undefined;
    vi.stubGlobal('PublicKeyCredential', class {});
    Object.defineProperty(navigator, 'credentials', {
      configurable: true,
      value: {
        get: vi.fn(async (options: CredentialRequestOptions) => {
          seen = options.publicKey;
          return {
            id: 'credential-1',
            type: 'public-key',
            response: {
              clientDataJSON: bytes(1).buffer,
              authenticatorData: bytes(2).buffer,
              signature: bytes(3, 4).buffer,
            },
          };
        }),
      },
    });

    const result = await getPasskeyAssertion({
      challenge_id: 'challenge-1',
      challenge: bytesToBase64Url(bytes(5, 5)),
      rp_id: 'karucode.example.com',
      timeout_ms: 60_000,
      allow: [{ id: 'credential-1', type: 'public-key' }],
    });

    expect(seen?.rpId).toBe('karucode.example.com');
    expect([...new Uint8Array(seen!.challenge as ArrayBuffer)]).toEqual([5, 5]);
    expect(seen?.allowCredentials).toHaveLength(1);
    expect(result).toEqual({
      credential_id: 'credential-1',
      client_data_json: bytesToBase64Url(bytes(1)),
      authenticator_data: bytesToBase64Url(bytes(2)),
      signature: bytesToBase64Url(bytes(3, 4)),
    });
  });
});
