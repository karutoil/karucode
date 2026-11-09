import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { passkeySupported } from '../lib/passkey';
import {
  AlertIcon,
  ArrowRightIcon,
  CheckIcon,
  EyeIcon,
  EyeOffIcon,
  KeyIcon,
  LockIcon,
} from './icons';

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
 *
 * The layout is two columns: what this thing *is* on the left, the form on the
 * right. A sign-in screen is the only page a person sees before they trust the
 * product, so it states where the code runs and who holds the keys rather than
 * presenting a bare password box - and on a narrow screen the identity column
 * folds down to the mark and the name, because the form is the job.
 */
export function AuthScreen(): JSX.Element {
  const [password, setPassword] = useState('');
  const [revealPassword, setRevealPassword] = useState(false);
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
  const tokenOnly =
    status !== null && status.token_required && !status.password_set && status.passkey_count === 0;
  const noMethod =
    status !== null && !status.password_set && status.passkey_count === 0 && !status.token_required;
  // When the status route cannot answer (an older server, a proxy eating the
  // request) the token form is the only method every version supports, so it is
  // shown rather than leaving the screen with nothing on it.
  const tokenVisible = tokenOnly || showToken || status === null;
  const address = status?.public_url ?? status?.origin ?? null;
  const addressIsTls = (status?.public_url ?? status?.origin ?? '').startsWith('https://');
  const sessionNote = status?.session_days
    ? `A sign-in lasts ${status.session_days} day${status.session_days === 1 ? '' : 's'} on this device.`
    : null;

  return (
    <main class="auth-page" aria-labelledby="auth-title">
      <div class="auth-shell">
        <aside class="auth-identity" aria-label="About karucode">
          <div class="auth-brand">
            <span class="auth-mark">
              <span class="auth-mark-glyph" />
            </span>
            <span class="auth-wordmark">karucode</span>
          </div>
          <p class="auth-tagline">A coding agent that runs on your machine.</p>
          <ul class="auth-points">
            <li>
              <span class="auth-point-icon">
                <LockIcon size={13} />
              </span>
              <span>
                <strong>Runs on this server.</strong> Your code stays on the machine you started it
                on.
              </span>
            </li>
            <li>
              <span class="auth-point-icon">
                <KeyIcon size={13} />
              </span>
              <span>
                <strong>Keys stay server-side.</strong> Model credentials are never sent to the
                browser.
              </span>
            </li>
            <li>
              <span class="auth-point-icon">
                <CheckIcon size={13} />
              </span>
              <span>
                <strong>You approve the changes.</strong> Shell commands and edits wait for you.
              </span>
            </li>
          </ul>
        </aside>

        <section class="auth-panel">
          <div class="auth-head">
            <span class="auth-mobile-brand" aria-hidden="true">
              <span class="auth-mark-glyph" />
              karucode
            </span>
            <h1 class="auth-title" id="auth-title">
              Sign in
            </h1>
            <p class="auth-subtitle">
              {address ? (
                <>
                  Continue to
                  <span class="auth-server" title={address}>
                    {addressIsTls ? <LockIcon size={11} /> : null}
                    <span class="auth-server-address">{address}</span>
                  </span>
                </>
              ) : (
                <>This server asks for a credential before it will answer.</>
              )}
            </p>
          </div>

          {error ? (
            <div class="auth-alert" role="alert">
              <AlertIcon size={15} />
              <span>{error}</span>
            </div>
          ) : null}

          {status === null ? (
            <p class="auth-resolving" aria-live="polite">
              Checking whether this server offers a password or passkey…
            </p>
          ) : null}

          {noMethod ? (
            <div class="auth-alert is-note" role="status">
              <AlertIcon size={15} />
              <span>
                Sign-in is required but no password or passkey is configured on the server. Set one
                from the server's Access settings, or start it with <code>KARUCODE_AUTH_TOKEN</code>.
              </span>
            </div>
          ) : null}

          {status?.password_set ? (
            <form
              class="auth-form"
              onSubmit={(event) => {
                event.preventDefault();
                void actions.signInWithPassword(password);
              }}
            >
              <div class="auth-field auth-field-with-reveal">
                <label class="auth-label" for="auth-password">
                  Password
                </label>
                <div class="auth-input-wrap">
                  <input
                    id="auth-password"
                    ref={passwordInput}
                    class="auth-input"
                    type={revealPassword ? 'text' : 'password'}
                    autoComplete="current-password"
                    value={password}
                    data-autofocus
                    disabled={pending}
                    aria-describedby="auth-password-hint"
                    onInput={(event) => setPassword(event.currentTarget.value)}
                  />
                  <button
                    type="button"
                    class="auth-reveal"
                    aria-label={revealPassword ? 'Hide password' : 'Show password'}
                    aria-pressed={revealPassword}
                    disabled={pending}
                    onClick={() => setRevealPassword((shown) => !shown)}
                  >
                    {revealPassword ? <EyeOffIcon size={15} /> : <EyeIcon size={15} />}
                  </button>
                </div>
                <p class="auth-hint" id="auth-password-hint">
                  The password you set in the Access settings.
                </p>
              </div>
              <button
                type="submit"
                class="button button-primary auth-submit"
                disabled={pending || password === ''}
              >
                {pending ? 'Signing in…' : 'Sign in'}
              </button>
            </form>
          ) : null}

          {passkeyOffered ? (
            <>
              {status?.password_set ? (
                <div class="auth-or">
                  <span>or</span>
                </div>
              ) : null}
              <button
                type="button"
                class="button button-secondary auth-passkey"
                disabled={pending}
                onClick={() => void actions.signInWithPasskey()}
              >
                <KeyIcon size={15} />
                {pending ? 'Waiting for your device…' : 'Sign in with a passkey'}
              </button>
              <p class="auth-hint auth-passkey-hint">
                Uses this device's fingerprint, face, screen lock or security key.
              </p>
            </>
          ) : null}

          {tokenVisible ? (
            <form
              class="auth-form auth-token-form"
              onSubmit={(event) => {
                event.preventDefault();
                setTokenPending(true);
                void actions.submitToken(token).finally(() => setTokenPending(false));
              }}
            >
              <div class="auth-field">
                <label class="auth-label" for="auth-token">
                  Access token
                </label>
                <div class="auth-input-wrap">
                  <input
                    id="auth-token"
                    ref={tokenInput}
                    class="auth-input mono"
                    type="password"
                    autoComplete="off"
                    spellcheck={false}
                    value={token}
                    disabled={tokenPending}
                    aria-describedby="auth-token-hint"
                    onInput={(event) => setToken(event.currentTarget.value)}
                  />
                </div>
                <p class="auth-hint" id="auth-token-hint">
                  The <code>KARUCODE_AUTH_TOKEN</code> of the machine running karucode. It is kept
                  in this browser and sent as an <code>Authorization</code> header.
                </p>
              </div>
              <button
                type="submit"
                class="button button-primary auth-submit"
                disabled={tokenPending || token.trim() === ''}
              >
                {tokenPending ? 'Connecting…' : 'Connect with the token'}
              </button>
            </form>
          ) : (
            <button
              type="button"
              class="auth-token-toggle"
              onClick={() => {
                setShowToken(true);
                store.authError.value = null;
              }}
            >
              Use the bearer token instead
              <ArrowRightIcon size={13} />
            </button>
          )}

          {sessionNote || status?.token_required ? (
            <p class="auth-note">
              {sessionNote}
              {status?.token_required ? (
                <>
                  {sessionNote ? ' ' : ''}A bearer token also works on this server.
                </>
              ) : null}
            </p>
          ) : null}
        </section>
      </div>
    </main>
  );
}
