import type { HumanInteractionAnswer, HumanInteractionRequestSnapshot } from '@mycopilot/protocol'
import {
  ChevronLeft,
  ChevronRight,
  MessageCircleQuestionMark,
  Minus,
  PencilLine
} from 'lucide-react'
import { useId } from 'react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import './HumanInteractionPanel.css'

export interface HumanInteractionPanelProps {
  request: Pick<HumanInteractionRequestSnapshot, 'requestId' | 'mode' | 'questions'>
  pageIndex: number
  answers: Readonly<Record<string, HumanInteractionAnswer | undefined>>
  canSubmit: boolean
  isSubmitting: boolean
  /** An uncertain mutation keeps the submitted payload immutable while allowing an exact retry. */
  isDraftLocked?: boolean
  sourceRunEnded?: boolean
  error?: string | null
  onPageChange: (pageIndex: number) => void
  onAnswerChange: (answer: HumanInteractionAnswer) => void
  onSubmit: () => void
  onIgnore?: () => void
  onMinimize?: () => void
}

/** Controlled presentation only. The owner preserves drafts and arbitrates every Host mutation. */
export function HumanInteractionPanel({
  request,
  pageIndex,
  answers,
  canSubmit,
  isSubmitting,
  isDraftLocked = false,
  sourceRunEnded = false,
  error,
  onPageChange,
  onAnswerChange,
  onSubmit,
  onIgnore,
  onMinimize
}: HumanInteractionPanelProps) {
  const { t } = useFrontendConfig()
  const titleId = useId()
  const questionId = useId()
  const textId = useId()
  const total = request.questions.length
  const currentPage = Math.max(0, Math.min(Number.isInteger(pageIndex) ? pageIndex : 0, total - 1))
  const question = request.questions[currentPage]
  if (!question) return null

  const answer = answers[question.id]
  const editingDisabled = isSubmitting || isDraftLocked
  const answered = request.questions.map((item) => {
    const value = answers[item.id]
    if (!value || value.questionId !== item.id) return false
    if (value.kind === 'text') return value.text.trim().length > 0
    if (value.kind === 'option') return item.options?.some((option) => option.id === value.optionId)
    return value.kind === 'skipped'
  })
  const complete = answered.every(Boolean)
  const primaryDisabled = complete
    ? isSubmitting || !canSubmit
    : editingDisabled || !answered[currentPage]

  const handlePrimaryAction = () => {
    if (primaryDisabled) return
    if (complete) {
      onSubmit()
      return
    }
    // Find the next unanswered question, including one skipped over with the top arrows.
    for (let offset = 1; offset < total; offset += 1) {
      const nextPage = (currentPage + offset) % total
      if (!answered[nextPage]) {
        onPageChange(nextPage)
        return
      }
    }
  }

  return (
    <section
      aria-busy={isSubmitting}
      aria-labelledby={titleId}
      className="human-interaction-panel"
      data-mode={request.mode}
      data-request-id={request.requestId}
      onKeyDown={(event) => {
        // Do not let composer/global bubble handlers interpret Enter, IME confirmation or Escape
        // as send, skip, approval or Stop.
        event.stopPropagation()
        if (event.key === 'Escape') event.preventDefault()
      }}
      onKeyUp={(event) => event.stopPropagation()}
      role="dialog"
    >
      <header className="human-interaction-panel__header">
        <h2 id={titleId}>
          <MessageCircleQuestionMark aria-hidden="true" />
          {t('humanInteraction.panel.title')}
        </h2>
        <nav
          aria-label={t('humanInteraction.panel.title')}
          className="human-interaction-panel__pages"
        >
          <button
            aria-label={t('humanInteraction.panel.previous')}
            disabled={currentPage === 0}
            onClick={() => onPageChange(currentPage - 1)}
            type="button"
          >
            <ChevronLeft aria-hidden="true" />
          </button>
          <span aria-live="polite" aria-atomic="true">
            {t('humanInteraction.panel.page')
              .replace('{current}', String(currentPage + 1))
              .replace('{total}', String(total))}
          </span>
          <button
            aria-label={t('humanInteraction.panel.next')}
            disabled={currentPage === total - 1}
            onClick={() => onPageChange(currentPage + 1)}
            type="button"
          >
            <ChevronRight aria-hidden="true" />
          </button>
        </nav>
        {request.mode === 'async' ? (
          <button
            aria-label={t('humanInteraction.panel.minimize')}
            className="human-interaction-panel__minimize"
            disabled={!onMinimize}
            onClick={onMinimize}
            title={t('humanInteraction.panel.minimize')}
            type="button"
          >
            <Minus aria-hidden="true" />
          </button>
        ) : null}
      </header>

      <div className="human-interaction-panel__body" key={question.id}>
        <h3 className="human-interaction-panel__question" id={questionId}>
          {question.title}
        </h3>
        {question.options?.length ? (
          <div
            aria-labelledby={questionId}
            className="human-interaction-panel__options"
            role="group"
          >
            {question.options.map((option, index) => (
              <button
                aria-pressed={answer?.kind === 'option' && answer.optionId === option.id}
                className="human-interaction-panel__option"
                disabled={editingDisabled}
                key={option.id}
                onClick={() =>
                  onAnswerChange({ kind: 'option', questionId: question.id, optionId: option.id })
                }
                type="button"
              >
                <span aria-hidden="true" className="human-interaction-panel__option-index">
                  {index + 1}
                </span>
                <span>{option.label}</span>
              </button>
            ))}
          </div>
        ) : null}
        <div className="human-interaction-panel__custom" data-selected={answer?.kind === 'text'}>
          <label htmlFor={textId}>
            <PencilLine aria-hidden="true" />
            {t('humanInteraction.panel.customAnswer')}
          </label>
          <input
            aria-describedby={questionId}
            disabled={editingDisabled}
            id={textId}
            onChange={(event) =>
              onAnswerChange({ kind: 'text', questionId: question.id, text: event.target.value })
            }
            onKeyDown={(event) => {
              if (
                event.key === 'Enter' &&
                !event.nativeEvent.isComposing &&
                event.nativeEvent.keyCode !== 229
              ) {
                // A single-line input must not implicitly submit an enclosing form.
                event.preventDefault()
              }
            }}
            placeholder={t('humanInteraction.panel.customPlaceholder')}
            type="text"
            value={answer?.kind === 'text' ? answer.text : ''}
          />
        </div>
      </div>

      {request.mode === 'async' && sourceRunEnded ? (
        <p className="human-interaction-panel__hint">{t('humanInteraction.panel.turnEnded')}</p>
      ) : null}
      {error ? (
        <p className="human-interaction-panel__error" role="alert">
          {error}
        </p>
      ) : null}

      <footer className="human-interaction-panel__footer">
        {request.mode === 'async' ? (
          <button
            className="human-interaction-panel__ignore"
            disabled={isSubmitting || !onIgnore}
            onClick={onIgnore}
            type="button"
          >
            {t('humanInteraction.panel.ignoreAll')}
          </button>
        ) : null}
        <div className="human-interaction-panel__actions">
          <button
            aria-pressed={answer?.kind === 'skipped'}
            className="human-interaction-panel__skip"
            disabled={editingDisabled}
            onClick={() => onAnswerChange({ kind: 'skipped', questionId: question.id })}
            type="button"
          >
            {t(
              answer?.kind === 'skipped'
                ? 'humanInteraction.panel.skipped'
                : 'humanInteraction.panel.skip'
            )}
          </button>
          <button
            className="human-interaction-panel__submit"
            data-state={primaryDisabled ? 'disabled' : 'ready'}
            disabled={primaryDisabled}
            onClick={handlePrimaryAction}
            type="button"
          >
            {t(complete ? 'humanInteraction.panel.submit' : 'humanInteraction.panel.next')}
          </button>
        </div>
      </footer>
    </section>
  )
}
