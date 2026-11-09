import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { passkeySupported } from '../lib/passkey';
import { AlertIcon, KeyIcon } from './icons';

/**
 * The sign-in screen.
 *
 * Three ways in, and the screen offers only the ones this server actually has:
 *
 * * a **password** (`password_set`),
 * * a **passkey** (`passkey_count > 0`, and only in a browser that has WebAuthn),
 * * the **bearer token**, as a fallback kept from before sign-in existed - a
 *   token-only deployment, or a server whose password was lost.
 *
 * The choices come from `GET /api/auth/status`, which is public precisely so this
 * screen can be drawn before any credential exists.
 */
export function AuthScreen(): JSX.Element {
  const [password, setPassword] = useState('');
  const [showToken, setShowToken] = useState(false);
  const [token, setToken] = useState('');
  const [tokenPending, setTokenPending] = useState(false);
  const status = store.authStatus.value;
  const error = store.authError.value;
  const pending = store.authPending.value;
  const passwordInput = useRef<HTMLInputElement>(null);
  const tokenInput = useRef<HTMLInputElement>(null);

  useEffect(() => {
    // Ask what this server offers; without it the screen would have to guess.
    void actions.refreshAuthStatus();
  }, []);

  useEffect(() => {
    if (status?.password_set) passwordInput.current?.focus();
    else if (showToken) tokenInput.current?.focus();
  }, [status?.password_set, showToken]);

  const passkeyOffered = (status?.passkey_count ?? 0) > 0 && passkeySupported();
  const tokenOnly = status !== null && status.token_required && !status.password_set && status.passkey_count === 0;
  const noMethod = status !== null && !status.password_set && status.passkey_count === 0 && !status.token_required;
  // When the status route cannot answer (an older server, a proxy eating the
  // request) the token form is the only method every version supports, so it is
  // shown rather than leaving the screen with nothing on it.
  const tokenVisible = tokenOnly || showToken || status === null;

  return (
    <div class="full-screen-state">
      <div class="state-card auth-card">
        <div class="brand brand-large">
          <span class="brand-dot" aria-hidden="true" />
          <span class="brand-name">karucode</span>
        </div>
        <h1 class="state-title">Sign in</h1>
        <p class="state-body">
          {status?.public_url ? (
            <>
              This server is reachable at <code>{status.public_url}</code>. Sign in to continue.
            </>
          ) : (
            <>This server asks for a credential before it will answer. Sign in to continue.</>
          )}
        </p>

        {error ? (
          <div class="state-error" role="alert">
            <AlertIcon size={15} />
            <span>{error}</span>
          </div>
        ) : null}

        {status === null && !tokenVisible ? (
          <p class="state-footnote" aria-live="polite">
            Checking how this server signs in…
          </p>
        ) : null}

        {status?.password_set ? (
          <form
            class="auth-form"
            onSubmit={(event) => {
              event.preventDefault();
              void actions.signInWithPassword(password);
            }}
          >
            <label class="field-label" for="auth-password">
              Password
            </label>
            <input
              id="auth-password"
              ref={passwordInput}
              class="input"
              type="password"
              autoComplete="current-password"
              value={password}
              data-autofocus
              disabled={pending}
              aria-describedby="auth-password-hint"
              onInput={(event) => setPassword(event.currentTarget.value)}
            />
            <p class="field-hint" id="auth-password-hint">
              The password you set in the Access settings.
            </p>
            <button type="submit" class="button button-primary" disabled={pending || password === ''}>
              {pending ? 'Signing in…' : 'Sign in'}
            </button>
          </form>
        ) : null}

        {passkeyOffered ? (
          <div class="auth-passkey">
            <button
              type="button"
              class="button button-secondary auth-passkey-button"
              disabled={pending}
              onClick={() => void actions.signInWithPasskey()}
            >
              <KeyIcon size={15} /> {pending ? 'Waiting for your device…' : 'Sign in with a passkey'}
            </button>
            <p class="field-hint">
              Uses this device's fingerprint, face, screen lock or security key.
            </p>
          </div>
        ) : null}

        {noMethod ? (
          <div class="state-error" role="status">
            <AlertIcon size={15} />
            <span>
              Sign-in is required but no password or passkey is configured on the server. Set one
              from the server's Access settings, or start it with <code>KARUCODE_AUTH_TOKEN</code>.
            </span>
          </div>
        ) : null}

        {tokenVisible ? null : (
          <button
            type="button"
            class="auth-token-toggle"
            onClick={() => {
              setShowToken(true);
              store.authError.value = null;
            }}
          >
            Use the bearer token instead
          </button>
        )}

        {tokenVisible ? (
          <form
            class="auth-form auth-token-form"
            onSubmit={(event) => {
              event.preventDefault();
              setTokenPending(true);
              void actions.submitToken(token).finally(() => setTokenPending(false));
            }}
          >
            <label class="field-label" for="auth-token">
              Access token
            </label>
            <input
              id="auth-token"
              ref={tokenInput}
              class="input mono"
              type="password"
              autoComplete="off"
              spellcheck={false}
              value={token}
              disabled={tokenPending}
              aria-describedby="auth-token-hint"
              onInput={(event) => setToken(event.currentTarget.value)}
            />
            <p class="field-hint" id="auth-token-hint">
              The <code>KARUCODE_AUTH_TOKEN</code> of the machine running karucode. It is kept in
              this browser and sent as an <code>Authorization</code> header.
            </p>
            <button
              type="submit"
              class="button button-primary"
              disabled={tokenPending || token.trim() === ''}
            >
              {tokenPending ? 'Connecting…' : 'Connect with the token'}
            </button>
          </form>
        ) : null}

        <p class="state-footnote">
          {status?.session_days
            ? `A sign-in lasts ${status.session_days} day${status.session_days === 1 ? '' : 's'} on this device.`
            : 'Your model provider key is separate and is stored on the server.'}
        </p>
      </div>
    </div>
  );
}
