import type { JSX } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { formatCount } from '../lib/format';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { CheckIcon, ChevronDownIcon } from './icons';

/**
 * Header control that switches the active provider profile + model.
 *
 * The selection is install-wide (the server stores it in settings), so switching
 * here changes the model for every project, not just the open one. Providers are
 * grouped, so two profiles that serve the same model id stay distinguishable.
 */
export function ModelSwitcher(): JSX.Element | null {
  const config = store.config.value;
  const activeProvider = store.activeProvider.value;
  const label = store.activeModelLabel.value;
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target;
      if (target instanceof Node && root.current && !root.current.contains(target)) {
        setOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        setOpen(false);
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);

  // Nothing to switch between (no profiles at all): stay out of the header.
  if (!config || !activeProvider || !label) return null;

  const choose = async (providerId: string, modelId: string): Promise<void> => {
    if (providerId === activeProvider.id && modelId === config.active.model) {
      setOpen(false);
      return;
    }
    setPending(`${providerId}:${modelId}`);
    try {
      const ok = await actions.switchModel(providerId, modelId);
      if (ok) setOpen(false);
    } finally {
      setPending(null);
    }
  };

  return (
    <div class="model-switcher" ref={root}>
      <button
        type="button"
        class="model-switcher-button"
        onClick={() => setOpen((value) => !value)}
        aria-haspopup="listbox"
        aria-expanded={open}
        title={`${activeProvider.name} · ${label} — switch model`}
        aria-label={`Active model: ${label} on ${activeProvider.name}. Switch model`}
      >
        <span class="model-switcher-text">
          <span class="model-switcher-model">{label}</span>
          <span class="model-switcher-provider">{activeProvider.name}</span>
        </span>
        <ChevronDownIcon size={12} />
      </button>

      {open ? (
        <div class="model-switcher-menu" role="listbox" aria-label="Choose a model">
          {config.providers.map((profile) => (
            <div class="model-switcher-group" key={profile.id}>
              {config.providers.length > 1 ? (
                <p class="model-switcher-group-title">
                  {profile.name}
                  {profile.api_key_set ? null : <span class="model-switcher-warn">no key</span>}
                </p>
              ) : null}
              {profile.models.length === 0 ? (
                <p class="model-switcher-empty">No models on this provider yet.</p>
              ) : null}
              {profile.models.map((model) => {
                const active = profile.id === activeProvider.id && model.id === config.active.model;
                const busy = pending === `${profile.id}:${model.id}`;
                return (
                  <button
                    type="button"
                    role="option"
                    aria-selected={active}
                    class={`model-switcher-option${active ? ' is-active' : ''}`}
                    key={`${profile.id}:${model.id}`}
                    disabled={busy}
                    onClick={() => void choose(profile.id, model.id)}
                  >
                    <span class="model-switcher-option-main">
                      <span class="model-switcher-option-name">{model.name?.trim() || model.id}</span>
                      {model.name?.trim() && model.name.trim() !== model.id ? (
                        <code class="model-switcher-option-id">{model.id}</code>
                      ) : null}
                    </span>
                    <span class="model-switcher-option-meta">
                      {busy ? (
                        'Switching…'
                      ) : (
                        <>
                          {model.context_window ? (
                            <span title="Context window">{formatCount(model.context_window)} ctx</span>
                          ) : null}
                          {model.tool_call === false ? <span title="No tool calling">no tools</span> : null}
                          {active ? <CheckIcon size={13} /> : null}
                        </>
                      )}
                    </span>
                  </button>
                );
              })}
            </div>
          ))}
          <div class="model-switcher-footer">
            <button
              type="button"
              class="link-button"
              onClick={() => {
                setOpen(false);
                actions.openSettings();
              }}
            >
              Manage providers and models
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}