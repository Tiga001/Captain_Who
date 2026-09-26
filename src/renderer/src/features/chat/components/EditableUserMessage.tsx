import { useEffect, useRef, useState } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { getUserFacingErrorMessage } from '../../../errors/userFacingError'

export function EditableUserMessage({
  hasAttachments,
  hasImageAttachments,
  initialContent,
  selectedModelAvailable,
  selectedModelSupportsImage,
  onCancel,
  onSubmit
}: {
  hasAttachments: boolean
  hasImageAttachments: boolean
  initialContent: string
  selectedModelAvailable: boolean
  selectedModelSupportsImage: boolean
  onCancel: () => void
  onSubmit: (content: string) => void | Promise<void>
}) {
  const { t } = useFrontendConfig()
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const [content, setContent] = useState(initialContent)
  const [error, setError] = useState<string | null>(null)
  const [isSubmitting, setIsSubmitting] = useState(false)
  const validationMessage = !selectedModelAvailable
    ? t('chat.noEnabledModels')
    : hasImageAttachments && !selectedModelSupportsImage
      ? t('chat.unsupportedImageWarning')
      : null
  const canSend = (content.trim().length > 0 || hasAttachments) && !validationMessage

  useEffect(() => {
    const textarea = textareaRef.current
    if (!textarea) return

    textarea.style.height = 'auto'
    textarea.style.height = `${textarea.scrollHeight}px`
  }, [content])

  const submit = async () => {
    if (!canSend || isSubmitting) return

    setIsSubmitting(true)
    setError(null)
    try {
      await onSubmit(content.trim())
    } catch (submitError) {
      setError(getUserFacingErrorMessage(submitError, t, 'chat.editMessageFailed'))
      setIsSubmitting(false)
    }
  }

  return (
    <form
      className="chat-message-edit"
      onSubmit={(event) => {
        event.preventDefault()
        void submit()
      }}
    >
      <textarea
        ref={textareaRef}
        aria-label={t('chat.editMessage')}
        autoFocus
        value={content}
        onChange={(event) => setContent(event.target.value)}
        onKeyDown={(event) => {
          if (event.key !== 'Enter' || event.shiftKey) return
          event.preventDefault()
          void submit()
        }}
      />
      {validationMessage && <p className="chat-message-edit__warning">{validationMessage}</p>}
      {error && <p className="chat-message-edit__error">{error}</p>}
      <div className="chat-message-edit__actions">
        <button disabled={isSubmitting} onClick={onCancel} type="button">
          {t('chat.cancel')}
        </button>
        <button disabled={!canSend || isSubmitting} type="submit">
          {t('chat.send')}
        </button>
      </div>
    </form>
  )
}
