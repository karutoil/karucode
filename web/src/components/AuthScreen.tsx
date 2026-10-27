import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { AlertIcon } from './icons';

export function AuthScreen(): JSX.Element {
  const [token, setToken] = useState('');
  const [pending, setPending] = useState(false);
  const tokenInput = useRef<HTMLInputElement>(null);
  const error = store.authError.value;

  useEffect(() => {
    tokenInput.current?.focus();
  }, []);

  return (
    <div class="full-screen-state">
      <div class="state-card auth-card">
        <div class="brand brand-large">
          <span class="brand-dot" aria-hidden="true" />
          <span class="brand-name">karucode</span>
        </div>
        <h1 class="state-title">Authentication required</h1>
        <p class="state-body">
          This server was started with a bearer token (<code>KARUCODE_AUTH_TOKEN</code>). Enter it to connect.
          The token is kept in this browser's local storage and sent as an <code>Authorization</code> header.
        </p>

        {error ? (
          <div class="state-error" role="alert">
            <AlertIcon size={15} />
            <span>{error}</span>
          </div>
        ) : null}

        <form
          class="auth-form"
          onSubmit={(event) => {
            event.preventDefault();
            setPending(true);
            void actions.submitToken(token).finally(() => setPending(false));
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
            data-autofocus
            disabled={pending}
            aria-describedby="auth-token-hint"
            onInput={(event) => setToken(event.currentTarget.value)}
          />
          <p class="field-hint" id="auth-token-hint">
            Find it in the environment of the machine running karucode.
          </p>
          <button type="submit" class="button button-primary" disabled={pending || token.trim() === ''}>
            {pending ? 'Connecting…' : 'Connect'}
          </button>
        </form>

        <p class="state-footnote">
          The API key for the model provider is separate and is managed in Settings after you connect.
        </p>
      </div>
    </div>
  );
}