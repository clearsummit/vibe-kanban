import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@vibe/ui/components/Dialog';
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

function statusColor(s: ClaudeAccountStatus): string {
  switch (s) {
    case 'active':
      return 'text-emerald-500';
    case 'throttled':
      return 'text-amber-500';
    case 'needs_reauth':
      return 'text-red-500';
    case 'disabled':
      return 'text-low';
  }
}

/** Localized status label. Hook needs to be called from a component. */
function useStatusLabel(s: ClaudeAccountStatus): string {
  const { t } = useTranslation('settings');
  return t(`settings.claude-accounts.status.${s}`);
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
  const { t } = useTranslation('settings');
  const [accounts, setAccounts] = useState<ClaudeAccountView[]>([]);
  const [policy, setPolicy] = useState<ClaudeRetryPolicy | null>(null);
  const [policyDraft, setPolicyDraft] = useState<ClaudeRetryPolicy | null>(
    null
  );
  const [loading, setLoading] = useState(true);
  const [policySaving, setPolicySaving] = useState(false);
  const [policyError, setPolicyError] = useState<string | null>(null);
  const [policySuccess, setPolicySuccess] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [addState, setAddState] = useState<AddAccountState | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const [a, p] = await Promise.all([
        claudeAccountsApi.list(),
        claudeAccountsApi.getRetryPolicy(),
      ]);
      setAccounts(a);
      setPolicy(p);
      setPolicyDraft(p);
    } catch (e) {
      // Don't render an empty-state on a network/API failure — that misleads
      // the user into thinking they have no enrolled accounts.
      setLoadError(e instanceof Error ? e.message : String(e));
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
        title: t('settings.claude-accounts.removeConfirm.title'),
        message: t('settings.claude-accounts.removeConfirm.message', {
          label: account.label,
        }),
        confirmText: t('settings.claude-accounts.removeConfirm.confirm'),
        variant: 'destructive',
      });
      if (result === 'confirmed') {
        await claudeAccountsApi.remove(account.id);
        await refresh();
      }
    },
    [refresh, t]
  );

  const renameAccount = useCallback(
    async (id: string, label: string) => {
      await claudeAccountsApi.update(id, { label, disabled: null });
      await refresh();
    },
    [refresh]
  );

  /// Wrap a possibly-rejecting async callback so we never leak unhandled
  /// promise rejections to the browser. On error, surface via the same
  /// `loadError` banner used by `refresh` failures so the user gets an
  /// actionable message instead of a silent crash.
  const safe = useCallback(
    <Args extends unknown[]>(fn: (...args: Args) => Promise<unknown>) => {
      return (...args: Args) => {
        fn(...args).catch((e) =>
          setLoadError(e instanceof Error ? e.message : String(e))
        );
      };
    },
    []
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
        title={t('settings.claude-accounts.title')}
        description={t('settings.claude-accounts.description')}
      >
        {loading ? (
          <div className="flex items-center gap-2 text-sm text-low">
            <SpinnerIcon className="animate-spin" />{' '}
            {t('settings.claude-accounts.loading')}
          </div>
        ) : loadError ? (
          <div className="space-y-3">
            <p className="text-sm text-red-500 flex items-center gap-1">
              <WarningIcon /> {t('settings.claude-accounts.loadError')}{' '}
              {loadError}
            </p>
            <button
              type="button"
              onClick={() => {
                void refresh();
              }}
              className="text-sm underline text-brand"
            >
              {t('settings.claude-accounts.retry')}
            </button>
          </div>
        ) : accounts.length === 0 ? (
          <div className="space-y-3">
            <p className="text-sm text-low">
              {t('settings.claude-accounts.emptyState')}
            </p>
            <PrimaryButton onClick={safe(() => startAdd(null))}>
              <PlusIcon /> {t('settings.claude-accounts.addAccount')}
            </PrimaryButton>
          </div>
        ) : (
          <div className="space-y-3 overflow-x-auto">
            <p className="text-xs text-low">
              {t('settings.claude-accounts.orderHint')}
            </p>
            <table className="w-full text-sm border-collapse">
              <thead>
                <tr className="text-left text-low border-b border-border">
                  <th className="py-2 pr-3 font-medium">
                    {t('settings.claude-accounts.columns.order')}
                  </th>
                  <th className="py-2 pr-3 font-medium">
                    {t('settings.claude-accounts.columns.account')}
                  </th>
                  <th className="py-2 pr-3 font-medium">
                    {t('settings.claude-accounts.columns.status')}
                  </th>
                  <th className="py-2 pr-3 font-medium">
                    {t('settings.claude-accounts.columns.fiveHourUsage')}
                  </th>
                  <th className="py-2 pr-3 font-medium">
                    {t('settings.claude-accounts.columns.fiveHourReset')}
                  </th>
                  <th className="py-2 pr-3 font-medium">
                    {t('settings.claude-accounts.columns.weeklyUsage')}
                  </th>
                  <th className="py-2 pr-3 font-medium">
                    {t('settings.claude-accounts.columns.weeklyReset')}
                  </th>
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
                    onMoveUp={safe(() => moveAccount(a.id, 'up'))}
                    onMoveDown={safe(() => moveAccount(a.id, 'down'))}
                    onRename={safe((label: string) =>
                      renameAccount(a.id, label)
                    )}
                    onToggleDisabled={safe(() => toggleDisabled(a))}
                    onRequestRemove={safe(() => removeAccount(a))}
                    onReauth={safe(() => startAdd(a.id))}
                  />
                ))}
              </tbody>
            </table>
            <PrimaryButton onClick={safe(() => startAdd(null))}>
              <PlusIcon /> {t('settings.claude-accounts.addAccount')}
            </PrimaryButton>
          </div>
        )}
      </SettingsCard>

      <SettingsCard
        title={t('settings.claude-accounts.retryPolicy.title')}
        description={t('settings.claude-accounts.retryPolicy.description')}
      >
        {policyDraft && (
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
            <SettingsField
              label={t('settings.claude-accounts.retryPolicy.maxAttempts')}
            >
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
                {t('settings.claude-accounts.retryPolicy.maxAttemptsHint')}
              </p>
            </SettingsField>
            <SettingsField
              label={t('settings.claude-accounts.retryPolicy.initialBackoff')}
            >
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
                {t('settings.claude-accounts.retryPolicy.initialBackoffHint')}
              </p>
            </SettingsField>
            <SettingsField
              label={t('settings.claude-accounts.retryPolicy.multiplier')}
            >
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
                {t('settings.claude-accounts.retryPolicy.multiplierHint')}
              </p>
            </SettingsField>
            <SettingsField
              label={t('settings.claude-accounts.retryPolicy.maxBackoff')}
            >
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
                {t('settings.claude-accounts.retryPolicy.maxBackoffHint')}
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
          <p className="text-sm text-emerald-500 mt-2">
            {t('settings.claude-accounts.retryPolicy.saved')}
          </p>
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
  const { t } = useTranslation('settings');
  const statusLabel = useStatusLabel(account.status);
  const statusColorCls = statusColor(account.status);
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
            aria-label={t('settings.claude-accounts.actions.moveUp')}
            disabled={!canMoveUp}
            onClick={onMoveUp}
            className="p-0.5 disabled:opacity-30 hover:text-normal transition-colors"
          >
            <ArrowUpIcon />
          </button>
          <button
            type="button"
            aria-label={t('settings.claude-accounts.actions.moveDown')}
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
      <td className={`py-3 pr-3 ${statusColorCls}`}>
        {statusLabel}
        {account.status === 'throttled' && throttleCountdown && (
          <div className="text-xs text-low">
            {t('settings.claude-accounts.status.throttledCountdown', {
              remaining: throttleCountdown,
            })}{' '}
            {account.throttle_reason === 'weekly'
              ? `(${t('settings.claude-accounts.status.throttledWeeklyCap')})`
              : `(${t('settings.claude-accounts.status.throttledFiveHourCap')})`}
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
              {t('settings.claude-accounts.actions.reauth')}
            </button>
          )}
          <button
            type="button"
            className="text-xs underline text-low"
            onClick={onToggleDisabled}
          >
            {account.status === 'disabled'
              ? t('settings.claude-accounts.actions.enable')
              : t('settings.claude-accounts.actions.disable')}
          </button>
          <button
            type="button"
            aria-label={t('settings.claude-accounts.actions.remove')}
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
  const { t } = useTranslation('settings');
  const title = state.accountId
    ? t('settings.claude-accounts.addModal.reauthTitle')
    : t('settings.claude-accounts.addModal.title');
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        // Radix invokes onOpenChange with `false` for Escape, outside-click,
        // and explicit close. Cancel the modal in all of those cases. Radix
        // also stops Escape from bubbling to the parent SettingsDialog, so
        // hitting Escape no longer closes the entire Settings dialog.
        if (!open) onCancel();
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <KeyIcon /> {title}
          </DialogTitle>
          <DialogDescription>
            {t('settings.claude-accounts.addModal.description')}
          </DialogDescription>
        </DialogHeader>
        <ol className="text-sm text-normal space-y-2 list-decimal pl-5">
          <li>
            {t('settings.claude-accounts.addModal.step1')}
            <div className="mt-1">
              <a
                href={state.authUrl}
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-1 text-brand underline text-sm"
              >
                {t('settings.claude-accounts.addModal.openClaude')}{' '}
                <ArrowSquareOutIcon />
              </a>
            </div>
          </li>
          <li>{t('settings.claude-accounts.addModal.step2')}</li>
          <li>{t('settings.claude-accounts.addModal.step3')}</li>
        </ol>
        <div>
          <label className="text-xs text-low block mb-1">
            {t('settings.claude-accounts.addModal.codeLabel')}
          </label>
          <input
            value={state.code}
            onChange={(e) => onCodeChange(e.target.value)}
            placeholder={t(
              'settings.claude-accounts.addModal.codePlaceholder'
            )}
            className="w-full bg-secondary/30 border border-border rounded px-2 py-1 text-sm font-mono"
            autoFocus
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
            {t('settings.claude-accounts.addModal.cancel')}
          </button>
          <PrimaryButton
            onClick={onSubmit}
            disabled={!state.code.trim() || state.exchanging}
          >
            {state.exchanging && <SpinnerIcon className="animate-spin" />}
            {t('settings.claude-accounts.addModal.complete')}
          </PrimaryButton>
        </div>
      </DialogContent>
    </Dialog>
  );
}
