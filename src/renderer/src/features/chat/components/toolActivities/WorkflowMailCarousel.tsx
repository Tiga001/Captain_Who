import { ChevronLeft, ChevronRight } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import './WorkflowMailCarousel.css'

/** One mounted carousel per tool call; closing its disclosure preserves the selection. */
export function WorkflowMailCarousel<T>({
  messages,
  chinese,
  messageKey,
  children
}: {
  messages: T[]
  chinese: boolean
  messageKey: (message: T, index: number) => string | number
  children: (message: T, index: number, navigation: ReactNode) => ReactNode
}) {
  const [selectedIndex, setSelectedIndex] = useState(0)
  const currentIndex = Math.max(0, Math.min(selectedIndex, messages.length - 1))
  if (selectedIndex !== currentIndex) setSelectedIndex(currentIndex)
  const message = messages[currentIndex]
  if (message === undefined) return null
  const previousLabel = chinese ? '上一封' : 'Previous message'
  const nextLabel = chinese ? '下一封' : 'Next message'
  const navigation = messages.length > 1 && (
    <nav
      className="workflow-mail-carousel__navigation"
      aria-label={chinese ? '切换邮件' : 'Messages'}
    >
      <button
        type="button"
        aria-label={previousLabel}
        title={previousLabel}
        disabled={currentIndex === 0}
        onClick={() => setSelectedIndex(currentIndex - 1)}
      >
        <ChevronLeft aria-hidden="true" />
      </button>
      <span aria-atomic="true" aria-live="polite">
        {currentIndex + 1} / {messages.length}
      </span>
      <button
        type="button"
        aria-label={nextLabel}
        title={nextLabel}
        disabled={currentIndex === messages.length - 1}
        onClick={() => setSelectedIndex(currentIndex + 1)}
      >
        <ChevronRight aria-hidden="true" />
      </button>
    </nav>
  )
  return (
    <div className="workflow-mail-carousel">
      <div key={messageKey(message, currentIndex)}>
        {children(message, currentIndex, navigation)}
      </div>
    </div>
  )
}
