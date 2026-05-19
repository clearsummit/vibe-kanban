import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowSquareOutIcon,
  ArrowUpIcon,
  ArrowDownIcon,
  KeyIcon,
  PlusIcon,
  SpinnerIcon,
  TrashIcon,
  WarningIcon,
} from '@phosphor-icons/react';
import { PrimaryButton } from '@vibe/ui/components/PrimaryButton';
import { ConfirmDialog } from '@vibe/ui/components/ConfirmDialog';
import { claudeAccountsApi } from '@/shared/lib/api';
import type {
  ClaudeAccountStatus,
  ClaudeAccountView,
  ClaudeRetryPolicy,
} from 'shared/types';
import {
  SettingsCard,
  SettingsField,
  SettingsInput,
  SettingsSaveBar,
} from './SettingsComponents';

interface AddAccountState {
  state: string;
  authUrl: string;
  code: string;
  exchanging: boolean;
  error: string | null;
  accountId: string | null; // when set, this is a re-auth in place
}

function describeStatus(s: ClaudeAccountStatus): {
  label: string;
  color: string;
} {
  switch (s) {
    case 'active':
      return { label: 'Active', color: 'text-emerald-500' };
    case 'throttled':
      return { label: 'Throttled', color: 'text-amber-500' };
    case 'needs_reauth':
      return { label: 'Needs re-auth', color: 'text-red-500' };
    case 'disabled':
      return { label: 'Disabled', color: 'text-low' };
  }
}

function formatLocal(ts: string | null | undefined): string {
  if (!ts) return '—';
  try {
    return new Date(ts).toLocaleString();
  } catch {
    return ts;
  }
}

function useCountdown(targetIso: string | null | undefined): string {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!targetIso) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [targetIso]);
  if (!targetIso) return '';
  const target = new Date(targetIso).getTime();
  const remaining = Math.max(0, target - now);
  if (remaining <= 0) return '0s';
  const seconds = Math.floor(remaining / 1000);
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

export function ClaudeAccountsSettingsSection() {
  const [accounts, setAccounts] = useState<ClaudeAccountView[]>([]);
  const [policy, setPolicy] = useState<ClaudeRetryPolicy | null>(null);
  const [policyDraft, setPolicyDraft] = useState<ClaudeRetryPolicy | null>(
    null
  );
  const [loading, setLoading] = useState(true);
  const [policySaving, setPolicySaving] = useState(false);
  const [policyError, setPolicyError] = useState<string | null>(null);
  const [policySuccess, setPolicySuccess] = useState(false);

  const [addState, setAddState] = useState<AddAccountState | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [a, p] = await Promise.all([
        claudeAccountsApi.list(),
        claudeAccountsApi.getRetryPolicy(),
      ]);
      setAccounts(a);
      setPolicy(p);
      setPolicyDraft(p);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const policyDirty = useMemo(() => {
    if (!policy || !policyDraft) return false;
    return (
      policy.max_attempts !== policyDraft.max_attempts ||
      policy.initial_backoff_seconds !== policyDraft.initial_backoff_seconds ||
      policy.backoff_multiplier !== policyDraft.backoff_multiplier ||
      policy.max_backoff_seconds !== policyDraft.max_backoff_seconds
    );
  }, [policy, policyDraft]);

  const savePolicy = useCallback(async () => {
    if (!policyDraft) return;
    setPolicySaving(true);
    setPolicyError(null);
    try {
      const saved = await claudeAccountsApi.putRetryPolicy(policyDraft);
      setPolicy(saved);
      setPolicyDraft(saved);
      setPolicySuccess(true);
      window.setTimeout(() => setPolicySuccess(false), 2000);
    } catch (e) {
      setPolicyError(e instanceof Error ? e.message : String(e));
    } finally {
      setPolicySaving(false);
    }
  }, [policyDraft]);

  const startAdd = useCallback(async (accountId: string | null) => {
    try {
      const resp = await claudeAccountsApi.startOAuth({
        account_id: accountId,
      });
      setAddState({
        state: resp.state,
        authUrl: resp.auth_url,
        code: '',
        exchanging: false,
        error: null,
        accountId,
      });
    } catch (e) {
      setAddState({
        state: '',
        authUrl: '',
        code: '',
        exchanging: false,
        error: e instanceof Error ? e.message : String(e),
        accountId,
      });
    }
  }, []);

  const completeAdd = useCallback(async () => {
    if (!addState) return;
    setAddState({ ...addState, exchanging: true, error: null });
    try {
      await claudeAccountsApi.completeOAuth({
        state: addState.state,
        code: addState.code.trim(),
      });
      setAddState(null);
      await refresh();
    } catch (e) {
      setAddState({
        ...addState,
        exchanging: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  }, [addState, refresh]);

  const toggleDisabled = useCallback(
    async (account: ClaudeAccountView) => {
      const target = account.status !== 'disabled';
      await claudeAccountsApi.update(account.id, {
        label: null,
        disabled: target,
      });
      await refresh();
    },
    [refresh]
  );

  const removeAccount = useCallback(
    async (account: ClaudeAccountView) => {
      const result = await ConfirmDialog.show({
        title: 'Remove account?',
        message: `Locally stored credentials for "${account.label}" will be deleted. You can re-enroll later by signing in again.`,
        confirmText: 'Remove',
        variant: 'destructive',
      });
      if (result === 'confirmed') {
        await claudeAccountsApi.remove(account.id);
        await refresh();
      }
    },
    [refresh]
  );

  const renameAccount = useCallback(
    async (id: string, label: string) => {
      await claudeAccountsApi.update(id, { label, disabled: null });
      await refresh();
    },
    [refresh]
  );

  const moveAccount = useCallback(
    async (id: string, direction: 'up' | 'down') => {
      const ids = accounts.map((a) => a.id);
      const idx = ids.indexOf(id);
      if (idx === -1) return;
      const swap = direction === 'up' ? idx - 1 : idx + 1;
      if (swap < 0 || swap >= ids.length) return;
      const next = [...ids];
      [next[idx], next[swap]] = [next[swap], next[idx]];
      const updated = await claudeAccountsApi.reorder({ order: next });
      setAccounts(updated);
    },
    [accounts]
  );

  return (
    <div className="space-y-6 pb-8">
      <SettingsCard
        title="Claude Accounts"
        description="Enroll multiple Claude OAuth accounts so Vibe Kanban can rotate when one hits its 5-hour or weekly usage cap."
      >
        {loading ? (
          <div className="flex items-center gap-2 text-sm text-low">
            <SpinnerIcon className="animate-spin" /> Loading…
          </div>
        ) : accounts.length === 0 ? (
          <div className="space-y-3">
            <p className="text-sm text-low">
              No Claude accounts enrolled yet. Vibe Kanban will fall back to the
              credentials in your ambient <code>~/.claude/</code> directory.
            </p>
            <PrimaryButton onClick={() => startAdd(null)}>
              <PlusIcon /> Add account
            </PrimaryButton>
          </div>
        ) : (
          <div className="space-y-3 overflow-x-auto">
            <p className="text-xs text-low">
              Rotation order: accounts higher in this list are picked first.
              Use the arrows to re-order.
            </p>
            <table className="w-full text-sm border-collapse">
              <thead>
                <tr className="text-left text-low border-b border-border">
                  <th className="py-2 pr-3 font-medium">Order</th>
                  <th className="py-2 pr-3 font-medium">Account</th>
                  <th className="py-2 pr-3 font-medium">Status</th>
                  <th className="py-2 pr-3 font-medium">5-hour usage</th>
                  <th className="py-2 pr-3 font-medium">5-hour reset</th>
                  <th className="py-2 pr-3 font-medium">Weekly usage</th>
                  <th className="py-2 pr-3 font-medium">Weekly reset</th>
                  <th className="py-2 pr-3 font-medium"></th>
                </tr>
              </thead>
              <tbody>
                {accounts.map((a, i) => (
                  <AccountRow
                    key={a.id}
                    account={a}
                    position={i + 1}
                    canMoveUp={i > 0}
                    canMoveDown={i < accounts.length - 1}
                    onMoveUp={() => moveAccount(a.id, 'up')}
                    onMoveDown={() => moveAccount(a.id, 'down')}
                    onRename={(label) => renameAccount(a.id, label)}
                    onToggleDisabled={() => toggleDisabled(a)}
                    onRequestRemove={() => removeAccount(a)}
                    onReauth={() => startAdd(a.id)}
                  />
                ))}
              </tbody>
            </table>
            <PrimaryButton onClick={() => startAdd(null)}>
              <PlusIcon /> Add account
            </PrimaryButton>
          </div>
        )}
      </SettingsCard>

      <SettingsCard
        title="Retry policy"
        description="When Claude returns a transient error (400/5xx/network), Vibe Kanban retries the same account with exponential back-off. When an account hits its 5-hour or weekly cap, Vibe Kanban rotates to the next healthy account. Set max_attempts to 0 to disable retry entirely."
      >
        {policyDraft && (
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
            <SettingsField label="Maximum retry attempts">
              <SettingsInput
                value={String(policyDraft.max_attempts)}
                onChange={(v) =>
                  setPolicyDraft({
                    ...policyDraft,
                    max_attempts: Math.max(0, Number(v) || 0),
                  })
                }
                placeholder="6"
              />
              <p className="text-xs text-low mt-1">
                Per task attempt. 0 disables retry.
              </p>
            </SettingsField>
            <SettingsField label="Initial back-off (seconds)">
              <SettingsInput
                value={String(policyDraft.initial_backoff_seconds)}
                onChange={(v) =>
                  setPolicyDraft({
                    ...policyDraft,
                    initial_backoff_seconds: Math.max(1, Number(v) || 1),
                  })
                }
                placeholder="30"
              />
              <p className="text-xs text-low mt-1">
                First retry delay. Default 30.
              </p>
            </SettingsField>
            <SettingsField label="Back-off multiplier">
              <SettingsInput
                value={String(policyDraft.backoff_multiplier)}
                onChange={(v) =>
                  setPolicyDraft({
                    ...policyDraft,
                    backoff_multiplier: Math.max(1, Number(v) || 1),
                  })
                }
                placeholder="2.0"
              />
              <p className="text-xs text-low mt-1">
                Each retry multiplies the delay. Default 2.0.
              </p>
            </SettingsField>
            <SettingsField label="Maximum back-off (seconds)">
              <SettingsInput
                value={String(policyDraft.max_backoff_seconds)}
                onChange={(v) =>
                  setPolicyDraft({
                    ...policyDraft,
                    max_backoff_seconds: Math.max(1, Number(v) || 1),
                  })
                }
                placeholder="300"
              />
              <p className="text-xs text-low mt-1">
                Hard cap on a single retry delay. Default 300.
              </p>
            </SettingsField>
          </div>
        )}
        {policyError && (
          <p className="text-sm text-red-500 mt-2 flex items-center gap-1">
            <WarningIcon /> {policyError}
          </p>
        )}
        {policySuccess && (
          <p className="text-sm text-emerald-500 mt-2">Saved.</p>
        )}
        <SettingsSaveBar
          show={policyDirty}
          saving={policySaving}
          onSave={() => {
            void savePolicy();
          }}
          onDiscard={() => setPolicyDraft(policy)}
        />
      </SettingsCard>

      {addState && (
        <AddAccountModal
          state={addState}
          onCodeChange={(code) => setAddState({ ...addState, code })}
          onSubmit={() => {
            void completeAdd();
          }}
          onCancel={() => setAddState(null)}
        />
      )}
    </div>
  );
}

function AccountRow({
  account,
  position,
  canMoveUp,
  canMoveDown,
  onMoveUp,
  onMoveDown,
  onRename,
  onToggleDisabled,
  onRequestRemove,
  onReauth,
}: {
  account: ClaudeAccountView;
  position: number;
  canMoveUp: boolean;
  canMoveDown: boolean;
  onMoveUp: () => void;
  onMoveDown: () => void;
  onRename: (label: string) => void;
  onToggleDisabled: () => void;
  onRequestRemove: () => void;
  onReauth: () => void;
}) {
  const status = describeStatus(account.status);
  const fiveHourCountdown = useCountdown(account.five_hour_window.reset_at);
  const weeklyCountdown = useCountdown(account.weekly_window.reset_at);
  const throttleCountdown = useCountdown(account.throttled_until);
  const [editing, setEditing] = useState(false);
  const [labelDraft, setLabelDraft] = useState(account.label);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    setLabelDraft(account.label);
  }, [account.label]);

  useEffect(() => {
    if (editing) inputRef.current?.focus();
  }, [editing]);

  const submitLabel = () => {
    setEditing(false);
    if (labelDraft.trim() && labelDraft !== account.label) {
      onRename(labelDraft.trim());
    } else {
      setLabelDraft(account.label);
    }
  };

  return (
    <tr className="border-b border-border last:border-b-0 align-middle">
      <td className="py-3 pr-3 whitespace-nowrap">
        <div className="flex items-center gap-1 text-low">
          <span className="font-mono tabular-nums text-xs w-5 text-right">
            {position}
          </span>
          <button
            type="button"
            aria-label="Move up in rotation order"
            disabled={!canMoveUp}
            onClick={onMoveUp}
            className="p-0.5 disabled:opacity-30 hover:text-normal transition-colors"
          >
            <ArrowUpIcon />
          </button>
          <button
            type="button"
            aria-label="Move down in rotation order"
            disabled={!canMoveDown}
            onClick={onMoveDown}
            className="p-0.5 disabled:opacity-30 hover:text-normal transition-colors"
          >
            <ArrowDownIcon />
          </button>
        </div>
      </td>
      <td className="py-3 pr-3">
        {editing ? (
          <input
            ref={inputRef}
            value={labelDraft}
            onChange={(e) => setLabelDraft(e.target.value)}
            onBlur={submitLabel}
            onKeyDown={(e) => {
              if (e.key === 'Enter') submitLabel();
              if (e.key === 'Escape') {
                setLabelDraft(account.label);
                setEditing(false);
              }
            }}
            className="bg-panel border border-border rounded px-2 py-1 text-sm w-full max-w-[200px]"
          />
        ) : (
          <button
            type="button"
            onClick={() => setEditing(true)}
            className="text-left hover:underline"
          >
            <div className="font-medium text-high">{account.label}</div>
            {account.email && (
              <div className="text-xs text-low">{account.email}</div>
            )}
          </button>
        )}
      </td>
      <td className={`py-3 pr-3 ${status.color}`}>
        {status.label}
        {account.status === 'throttled' && throttleCountdown && (
          <div className="text-xs text-low">
            resets in {throttleCountdown}
            {account.throttle_reason === 'weekly'
              ? ' (weekly cap)'
              : ' (5h cap)'}
          </div>
        )}
      </td>
      <td className="py-3 pr-3">{account.five_hour_window.used}</td>
      <td className="py-3 pr-3 text-low">
        {formatLocal(account.five_hour_window.reset_at)}
        {fiveHourCountdown && (
          <div className="text-xs">in {fiveHourCountdown}</div>
        )}
      </td>
      <td className="py-3 pr-3">{account.weekly_window.used}</td>
      <td className="py-3 pr-3 text-low">
        {formatLocal(account.weekly_window.reset_at)}
        {weeklyCountdown && <div className="text-xs">in {weeklyCountdown}</div>}
      </td>
      <td className="py-3 pr-3">
        <div className="flex items-center gap-2">
          {account.status === 'needs_reauth' && (
            <button
              type="button"
              className="text-xs underline text-brand"
              onClick={onReauth}
            >
              Re-auth
            </button>
          )}
          <button
            type="button"
            className="text-xs underline text-low"
            onClick={onToggleDisabled}
          >
            {account.status === 'disabled' ? 'Enable' : 'Disable'}
          </button>
          <button
            type="button"
            aria-label="Remove account"
            onClick={onRequestRemove}
            className="p-1 text-low hover:text-red-500 transition-colors"
          >
            <TrashIcon />
          </button>
        </div>
      </td>
    </tr>
  );
}

function AddAccountModal({
  state,
  onCodeChange,
  onSubmit,
  onCancel,
}: {
  state: AddAccountState;
  onCodeChange: (v: string) => void;
  onSubmit: () => void;
  onCancel: () => void;
}) {
  const heading = state.accountId
    ? 'Re-authorize account'
    : 'Add Claude account';
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
      <div className="bg-panel border border-border rounded-md w-full max-w-md p-5 space-y-4">
        <div className="flex items-center gap-2">
          <KeyIcon />
          <h3 className="font-semibold text-high">{heading}</h3>
        </div>
        <ol className="text-sm text-normal space-y-2 list-decimal pl-5">
          <li>
            Click the button below to open claude.ai and sign in.
            <div className="mt-1">
              <a
                href={state.authUrl}
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-1 text-brand underline text-sm"
              >
                Open claude.ai <ArrowSquareOutIcon />
              </a>
            </div>
          </li>
          <li>Authorize Vibe Kanban; claude.ai will display a code.</li>
          <li>Paste the code here and click Complete.</li>
        </ol>
        <div>
          <label className="text-xs text-low block mb-1">
            Code from claude.ai
          </label>
          <input
            value={state.code}
            onChange={(e) => onCodeChange(e.target.value)}
            placeholder="paste-the-code-here"
            className="w-full bg-secondary/30 border border-border rounded px-2 py-1 text-sm font-mono"
          />
        </div>
        {state.error && (
          <p className="text-sm text-red-500 flex items-center gap-1">
            <WarningIcon /> {state.error}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={onCancel}
            className="text-sm text-low hover:text-normal px-3 py-1.5"
          >
            Cancel
          </button>
          <PrimaryButton
            onClick={onSubmit}
            disabled={!state.code.trim() || state.exchanging}
          >
            {state.exchanging && <SpinnerIcon className="animate-spin" />}
            Complete enrollment
          </PrimaryButton>
        </div>
      </div>
    </div>
  );
}
