import { AlertCircle, ArrowUpRight, CheckCircle2, Clock3, LoaderCircle, Play } from 'lucide-react'
import { useEffect, useRef } from 'react'
import type { AutomationRun } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { useLicense } from '../../license/LicenseContext'
import { useAccountAuth } from '../../auth/AccountAuthContext'
import { getTurnAccessErrorCode } from '../../license/turnAccessError'
import {
  formatAbsoluteDateTime,
  runErrorMessage,
  runStatusLabel,
  triggerLabel
} from '../automationPresentation'

interface AutomationRunHistoryProps {
  error?: Error | null
  focusRunId?: string | null
  hasMore?: boolean
  loading?: boolean
  loadingMore?: boolean
  onAcknowledge: (attentionId: string) => void
  onLoadMore: () => void
  onOpenConversation: (conversationId: string, messageId?: string | null) => void
  onRetry: () => void
  runs: readonly AutomationRun[]
}

export function AutomationRunHistory({
  error,
  focusRunId,
  hasMore = false,
  loading = false,
  loadingMore = false,
  onAcknowledge,
  onLoadMore,
  onOpenConversation,
  onRetry,
  runs
}: AutomationRunHistoryProps) {
  const { language, t } = useFrontendConfig()
  const license = useLicense()
  const auth = useAccountAuth()
  const focusedRunRef = useRef<HTMLLIElement>(null)

  useEffect(() => {
    if (!focusRunId || !focusedRunRef.current) return
    focusedRunRef.current.scrollIntoView({ block: 'nearest' })
    focusedRunRef.current.focus({ preventScroll: true })
  }, [focusRunId, runs])

  return (
    <section className="automation-run-history" aria-labelledby="automation-run-history-heading">
      <h3 id="automation-run-history-heading">{t('automation.runHistory')}</h3>
      {loading && runs.length === 0 ? (
        <div className="automation-run-history__loading" role="status">
          <LoaderCircle className="automation-spinning" aria-hidden="true" />
          <span>{t('automation.loading')}</span>
        </div>
      ) : error && runs.length === 0 ? (
        <div className="automation-run-history__empty" role="alert">
          <p>{t('automation.historyLoadFailed')}</p>
          <button type="button" onClick={onRetry}>
            {t('automation.retry')}
          </button>
        </div>
      ) : runs.length === 0 ? (
        <p className="automation-run-history__empty">{t('automation.noRuns')}</p>
      ) : (
        <ul>
          {runs.map((run) => {
            const inProgress =
              run.status === 'queued' || run.status === 'starting' || run.status === 'running'
            const RunIcon =
              run.status === 'failed' || run.status === 'waiting_for_approval'
                ? AlertCircle
                : run.status === 'completed'
                  ? CheckCircle2
                  : inProgress
                    ? LoaderCircle
                    : run.status === 'cancelled'
                      ? Clock3
                      : Play
            const messageId = run.assistantMessageId ?? run.userMessageId
            const errorMessage = runErrorMessage(t, run)
            const accessError = getTurnAccessErrorCode({ code: run.errorCode })
            return (
              <li
                key={run.runId}
                ref={run.runId === focusRunId ? focusedRunRef : undefined}
                className="automation-run-history__item"
                data-attention={Boolean(run.attention) || undefined}
                data-focus={run.runId === focusRunId || undefined}
                data-status={run.status}
                tabIndex={run.runId === focusRunId ? -1 : undefined}
              >
                <RunIcon
                  className={inProgress ? 'automation-spinning' : undefined}
                  aria-hidden="true"
                />
                <div className="automation-run-history__content">
                  <div className="automation-run-history__title">
                    <strong>
                      {accessError ? t('license.runNotExecuted') : runStatusLabel(t, run.status)}
                    </strong>
                    <span>{triggerLabel(t, run.triggerKind)}</span>
                    <time dateTime={new Date(run.createdAt).toISOString()}>
                      {formatAbsoluteDateTime(run.createdAt, language)}
                    </time>
                  </div>
                  {run.resultPreview && <p>{run.resultPreview}</p>}
                  {errorMessage && <p className="automation-run-history__error">{errorMessage}</p>}
                  <div className="automation-run-history__actions">
                    {accessError ? (
                      <button
                        type="button"
                        onClick={() => {
                          // Only the explicit action may show login or leave the app. A new
                          // history record and restored access never replay this occurrence.
                          if (auth?.state.status !== 'signedIn') auth?.requestLogin()
                          else if (accessError === 'ACCOUNT_LICENSE_UNAVAILABLE')
                            void license?.refresh()
                          else license?.requestAccess()
                        }}
                      >
                        {t(
                          auth?.state.status !== 'signedIn'
                            ? 'auth.login'
                            : accessError === 'ACCOUNT_LICENSE_UNAVAILABLE'
                              ? 'license.retry'
                              : 'license.manage'
                        )}
                      </button>
                    ) : null}
                    {run.conversationId && (
                      <button
                        type="button"
                        onClick={() => {
                          if (run.attention?.readAt === null) {
                            onAcknowledge(run.attention.attentionId)
                          }
                          onOpenConversation(run.conversationId!, messageId)
                        }}
                      >
                        <span>{t('automation.openChat')}</span>
                        <ArrowUpRight aria-hidden="true" />
                      </button>
                    )}
                    {run.attention && run.attention.readAt === null && (
                      <button
                        type="button"
                        onClick={() => onAcknowledge(run.attention!.attentionId)}
                      >
                        {t('automation.acknowledgeAttention')}
                      </button>
                    )}
                  </div>
                </div>
              </li>
            )
          })}
        </ul>
      )}
      {hasMore && (
        <button
          type="button"
          className="automation-run-history__more"
          disabled={loadingMore}
          onClick={onLoadMore}
        >
          {loadingMore ? t('automation.loading') : t('automation.loadMore')}
        </button>
      )}
    </section>
  )
}
