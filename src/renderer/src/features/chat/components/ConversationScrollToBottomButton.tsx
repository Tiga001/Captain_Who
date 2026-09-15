// Renderer chat: floating control that returns the reader to the newest messages.
import { ArrowDown } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import './ConversationScrollToBottomButton.css'

interface ConversationScrollToBottomButtonProps {
  /** Swaps the arrow for a typing ellipsis while an assistant reply is still streaming. */
  generating: boolean
  visible: boolean
  onClick: () => void
}

export function ConversationScrollToBottomButton({
  generating,
  visible,
  onClick
}: ConversationScrollToBottomButtonProps) {
  const { t } = useFrontendConfig()

  if (!visible) return null

  return (
    <button
      aria-label={t('chat.scrollToBottom')}
      className="conversation-scroll-to-bottom"
      data-generating={generating ? 'true' : undefined}
      onClick={onClick}
      type="button"
    >
      {generating ? (
        <span aria-hidden="true" className="conversation-scroll-to-bottom__typing">
          <span />
          <span />
          <span />
        </span>
      ) : (
        <ArrowDown aria-hidden="true" className="conversation-scroll-to-bottom__icon" />
      )}
    </button>
  )
}
