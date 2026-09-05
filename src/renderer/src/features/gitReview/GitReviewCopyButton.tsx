import { useEffect, useState } from 'react'
import { Check, Copy } from 'lucide-react'
import { Tooltip } from '../../components/overlay/Tooltip'
import { useToast } from '../../components/toast/ToastContext'
import type { Translate } from '../../config/translationFormat'

interface GitReviewCopyButtonProps {
  anchorClassName?: string
  disabled?: boolean
  label: string
  onCopy: () => Promise<void>
  t: Translate
}

export function GitReviewCopyButton({
  anchorClassName = 'git-review__file-action-wrap',
  disabled = false,
  label,
  onCopy,
  t
}: GitReviewCopyButtonProps) {
  const { showToast } = useToast()
  const [status, setStatus] = useState<'idle' | 'copying' | 'copied'>('idle')

  useEffect(() => {
    if (status !== 'copied') return undefined
    const timer = window.setTimeout(() => setStatus('idle'), 1600)
    return () => window.clearTimeout(timer)
  }, [status])

  const copy = async (): Promise<void> => {
    if (disabled || status === 'copying') return
    setStatus('copying')
    try {
      await onCopy()
      setStatus('copied')
      showToast(t('gitReview.copy.success'))
    } catch {
      setStatus('idle')
      showToast(t('gitReview.copy.failed'))
    }
  }

  return (
    <Tooltip
      anchorClassName={anchorClassName}
      content={status === 'copied' ? t('gitReview.copy.success') : label}
    >
      <button
        aria-busy={status === 'copying' || undefined}
        aria-label={label}
        className="git-review__file-action-button"
        disabled={disabled || status === 'copying'}
        onClick={(event) => {
          event.stopPropagation()
          void copy()
        }}
        type="button"
      >
        {status === 'copied' ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
      </button>
    </Tooltip>
  )
}
