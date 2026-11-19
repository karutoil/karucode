import { useState } from 'preact/hooks';
import { ApiError, toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import type { ApiKeyScope, ApiKeySummary } from '../../api/types';
import { copyText } from '../../lib/clipboard';
import {
  DEFAULT_KEY_SCOPES,
  type IssuedKey,
  MAX_KEY_LIFETIME_DAYS,
  type Message,
} from './helpers';

export interface UseApiKeysResult {
  keyBusy: string | null;
  keyMessage: Message | null;
  issuedKey: IssuedKey | null;
  newKeyName: string;
  newKeyScopes: ApiKeyScope[];
  newKeyExpiry: string;
  adminToken: string;
  confirmRevokeKey: string | null;
  renamingKey: { id: string; name: string } | null;
  copiedKey: boolean;
  setNewKeyName: (name: string) => void;
  setNewKeyExpiry: (expiry: string) => void;
  setAdminToken: (token: string) => void;
  setConfirmRevokeKey: (id: string | null) => void;
  setRenamingKey: (renaming: { id: string; name: string } | null) => void;
  dismissIssuedKey: () => void;
  toggleKeyScope: (scope: ApiKeyScope) => void;
  createKey: () => Promise<void>;
  copyIssuedKey: () => Promise<void>;
  rotateKey: (key: ApiKeySummary) => Promise<void>;
  revokeKey: (key: ApiKeySummary) => Promise<void>;
  purgeKey: (key: ApiKeySummary) => Promise<void>;
  saveKeyRename: (id: string, name: string) => Promise<void>;
}

export function useApiKeys(refreshApiKeys: () => Promise<void>): UseApiKeysResult {
  // API keys. `issuedKey` is the only place a secret ever lives in this
  // component: the server shows it once, and dismissing the panel is permanent.
  const [keyBusy, setKeyBusy] = useState<string | null>(null);
  const [keyMessage, setKeyMessage] = useState<Message | null>(null);
  const [issuedKey, setIssuedKey] = useState<IssuedKey | null>(null);
  const [newKeyName, setNewKeyName] = useState('');
  const [newKeyScopes, setNewKeyScopes] = useState<ApiKeyScope[]>([...DEFAULT_KEY_SCOPES]);
  const [newKeyExpiry, setNewKeyExpiry] = useState('30');
  const [adminToken, setAdminToken] = useState('');
  const [confirmRevokeKey, setConfirmRevokeKey] = useState<string | null>(null);
  const [renamingKey, setRenamingKey] = useState<{ id: string; name: string } | null>(null);
  const [copiedKey, setCopiedKey] = useState(false);

  /** Add or remove one scope from the draft key. */
  const toggleKeyScope = (scope: ApiKeyScope): void => {
    setNewKeyScopes((current) =>
      current.includes(scope)
        ? current.filter((entry) => entry !== scope)
        : [...current, scope],
    );
  };

  /**
   * Create a key and surface the one copy of its token.
   *
   * The expiry is a whole number of days or empty (never expires). The server is
   * the authority on both bounds; the checks here exist so a typo is answered
   * before a round trip, not so the form can disagree with it.
   *
   * The POST is not idempotent and the answer is the only copy of the secret, so
   * a timeout (or a dropped connection) is ambiguous: the key may already exist
   * on the server while its token is gone for good. That failure is never left
   * as a bare error next to a still-armed "Create key" button, because the blind
   * retry the hint invites would mint a second key and orphan the first. Instead
   * the list is re-read so a minted key shows up in it, and the message says
   * what actually happened and what to do: rotate the key if it is there.
   */
  const createKey = async (): Promise<void> => {
    if (keyBusy !== null) return;
    const name = newKeyName.trim();
    if (name === '') {
      setKeyMessage({ ok: false, text: 'Give the key a name so you can recognise it later.' });
      return;
    }
    const rawDays = newKeyExpiry.trim();
    const days = rawDays === '' ? undefined : Number(rawDays);
    if (
      days !== undefined &&
      (!Number.isInteger(days) || days < 1 || days > MAX_KEY_LIFETIME_DAYS)
    ) {
      setKeyMessage({
        ok: false,
        text: `Use a whole number of days from 1 to ${MAX_KEY_LIFETIME_DAYS}, or leave it empty for no expiry.`,
      });
      return;
    }
    if (newKeyScopes.length === 0) {
      setKeyMessage({ ok: false, text: 'Choose at least one scope for the key.' });
      return;
    }
    setKeyBusy('create');
    setKeyMessage(null);
    setCopiedKey(false);
    try {
      const issued = await api.createApiKey(
        {
          name,
          scopes: newKeyScopes,
          ...(days !== undefined ? { expires_in_days: days } : {}),
        },
        adminToken,
      );
      setIssuedKey({
        id: issued.api_key.id,
        name: issued.api_key.name,
        token: issued.token,
        warning: issued.warning,
      });
      setNewKeyName('');
      await refreshApiKeys();
      setKeyMessage({
        ok: true,
        text: `Key “${issued.api_key.name}” created. Copy the token now — it is not shown again.`,
      });
    } catch (error) {
      if (error instanceof ApiError && (error.isNetwork || error.code === 'timeout')) {
        // Ambiguous: the server may have minted the key and lost only the
        // answer, which is the sole copy of the token. Re-read the list so the
        // key (if it exists) is on screen, and say what to do next.
        await refreshApiKeys();
        setKeyMessage({
          ok: false,
          text: `Key “${name}” may already have been created — the server’s answer (the only copy of its token) was lost. Check the list: if the key is there, rotate it to get a new token; only create it again if it is not.`,
        });
      } else {
        setKeyMessage({ ok: false, text: toErrorMessage(error) });
      }
    } finally {
      setKeyBusy(null);
    }
  };

  const copyIssuedKey = async (): Promise<void> => {
    if (issuedKey === null) return;
    const copied = await copyText(issuedKey.token);
    setCopiedKey(copied);
    setKeyMessage(
      copied
        ? { ok: true, text: 'Token copied to the clipboard.' }
        : { ok: false, text: 'The browser refused to copy it; select the token and copy it by hand.' },
    );
  };

  const rotateKey = async (key: ApiKeySummary): Promise<void> => {
    if (keyBusy !== null) return;
    setKeyBusy(key.id);
    setKeyMessage(null);
    setCopiedKey(false);
    try {
      const issued = await api.rotateApiKey(key.id, adminToken);
      setIssuedKey({
        id: issued.api_key.id,
        name: issued.api_key.name,
        token: issued.token,
        warning: issued.warning,
      });
      await refreshApiKeys();
      setKeyMessage({
        ok: true,
        text: `Key “${issued.api_key.name}” rotated. The old secret stopped working; copy the new one now.`,
      });
    } catch (error) {
      setKeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setKeyBusy(null);
    }
  };

  const revokeKey = async (key: ApiKeySummary): Promise<void> => {
    if (keyBusy !== null) return;
    setKeyBusy(key.id);
    setKeyMessage(null);
    try {
      await api.revokeApiKey(key.id, false, adminToken);
      setConfirmRevokeKey(null);
      // A revoked key's token is dead; do not leave it on screen as if it were
      // still usable.
      if (issuedKey !== null && issuedKey.id === key.id) {
        setIssuedKey(null);
        setCopiedKey(false);
      }
      await refreshApiKeys();
      setKeyMessage({
        ok: true,
        text: `Key “${key.name}” revoked. It cannot authenticate again.`,
      });
    } catch (error) {
      setKeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setKeyBusy(null);
    }
  };

  /** Remove a revoked or expired key's row for good. */
  const purgeKey = async (key: ApiKeySummary): Promise<void> => {
    if (keyBusy !== null) return;
    setKeyBusy(key.id);
    setKeyMessage(null);
    try {
      await api.revokeApiKey(key.id, true, adminToken);
      if (issuedKey !== null && issuedKey.id === key.id) {
        setIssuedKey(null);
        setCopiedKey(false);
      }
      await refreshApiKeys();
      setKeyMessage({ ok: true, text: `Key “${key.name}” removed from the list.` });
    } catch (error) {
      setKeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setKeyBusy(null);
    }
  };

  const saveKeyRename = async (id: string, name: string): Promise<void> => {
    const trimmed = name.trim();
    if (trimmed === '') {
      setKeyMessage({ ok: false, text: 'A key needs a name.' });
      return;
    }
    if (keyBusy !== null) return;
    setKeyBusy(id);
    setKeyMessage(null);
    try {
      await api.updateApiKey(id, { name: trimmed }, adminToken);
      setRenamingKey(null);
      await refreshApiKeys();
      setKeyMessage({ ok: true, text: `Key renamed to “${trimmed}”.` });
    } catch (error) {
      setKeyMessage({ ok: false, text: toErrorMessage(error) });
    } finally {
      setKeyBusy(null);
    }
  };

  const dismissIssuedKey = (): void => {
    setIssuedKey(null);
    setCopiedKey(false);
  };

  return {
    keyBusy,
    keyMessage,
    issuedKey,
    newKeyName,
    newKeyScopes,
    newKeyExpiry,
    adminToken,
    confirmRevokeKey,
    renamingKey,
    copiedKey,
    setNewKeyName,
    setNewKeyExpiry,
    setAdminToken,
    setConfirmRevokeKey,
    setRenamingKey,
    dismissIssuedKey,
    toggleKeyScope,
    createKey,
    copyIssuedKey,
    rotateKey,
    revokeKey,
    purgeKey,
    saveKeyRename,
  };
}
