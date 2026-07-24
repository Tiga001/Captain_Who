import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type DragEvent,
  type KeyboardEvent
} from 'react'
import {
  CornerDownRight,
  GripVertical,
  MoreHorizontal,
  PanelRightOpen,
  Pencil,
  Trash2
} from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { stripAttachmentSummary } from '../chatAttachments'
import type { ChatQueuedMessage } from '../chatTypes'

interface GuidanceQueueProps {
  guideEnabled: boolean
  messages: ChatQueuedMessage[]
  onDelete: (messageId: string) => void
  onEdit: (messageId: string) => void
  onGuide: (message: ChatQueuedMessage) => void
  onMove: (messageId: string, targetMessageId: string) => void
  onOpenSideChat: (message: ChatQueuedMessage) => void
}

export function GuidanceQueue({
  guideEnabled,
  messages,
  onDelete,
  onEdit,
  onGuide,
  onMove,
  onOpenSideChat
}: GuidanceQueueProps) {
  const { t } = useFrontendConfig()
  const [openMenuId, setOpenMenuId] = useState<string | null>(null)
  const [draggingId, setDraggingId] = useState<string | null>(null)
  const [dragOverId, setDragOverId] = useState<string | null>(null)
  const menuHostRef = useRef<HTMLDivElement>(null)
  const itemRefs = useRef(new Map<string, HTMLDivElement>())
  const previousItemRectsRef = useRef(new Map<string, DOMRect>())
  const itemAnimationsRef = useRef(new Map<string, Animation>())
  const lastDragTargetRef = useRef<string | null>(null)

  useEffect(() => {
    if (!openMenuId) return undefined
    const closeMenu = (event: PointerEvent) => {
      const target = event.target
      if (target instanceof Node && menuHostRef.current?.contains(target)) return
      setOpenMenuId(null)
    }
    window.addEventListener('pointerdown', closeMenu)
    return () => window.removeEventListener('pointerdown', closeMenu)
  }, [openMenuId])

  useEffect(
    () => () => {
      itemAnimationsRef.current.forEach((animation) => animation.cancel())
      itemAnimationsRef.current.clear()
    },
    []
  )

  useLayoutEffect(() => {
    const previousRects = previousItemRectsRef.current
    previousItemRectsRef.current = new Map()
    if (previousRects.size === 0 || window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      return
    }

    itemRefs.current.forEach((element, messageId) => {
      if (messageId === draggingId) return
      const previousRect = previousRects.get(messageId)
      if (!previousRect) return
      const deltaY = previousRect.top - element.getBoundingClientRect().top
      if (Math.abs(deltaY) < 1) return

      itemAnimationsRef.current.get(messageId)?.cancel()
      element.dataset.reordering = 'true'
      const animation = element.animate(
        [{ transform: `translateY(${deltaY}px)` }, { transform: 'translateY(0)' }],
        {
          duration: 180,
          easing: 'cubic-bezier(0.2, 0.8, 0.2, 1)'
        }
      )
      itemAnimationsRef.current.set(messageId, animation)
      const finishAnimation = () => {
        if (itemAnimationsRef.current.get(messageId) === animation) {
          itemAnimationsRef.current.delete(messageId)
          delete element.dataset.reordering
        }
      }
      animation.addEventListener('finish', finishAnimation, { once: true })
      animation.addEventListener('cancel', finishAnimation, { once: true })
    })
  }, [draggingId, messages])

  if (messages.length === 0) return null

  const captureItemPositions = () => {
    previousItemRectsRef.current = new Map(
      Array.from(itemRefs.current, ([messageId, element]) => [
        messageId,
        element.getBoundingClientRect()
      ])
    )
  }

  const finishDragging = () => {
    setDraggingId(null)
    setDragOverId(null)
    lastDragTargetRef.current = null
  }

  const moveWithKeyboard = (
    event: KeyboardEvent<HTMLButtonElement>,
    message: ChatQueuedMessage,
    index: number
  ) => {
    const targetIndex =
      event.key === 'ArrowUp'
        ? Math.max(0, index - 1)
        : event.key === 'ArrowDown'
          ? Math.min(messages.length - 1, index + 1)
          : index
    if (targetIndex === index) return
    event.preventDefault()
    captureItemPositions()
    onMove(message.id, messages[targetIndex].id)
  }

  const startDragging = (event: DragEvent<HTMLButtonElement>, messageId: string) => {
    setDraggingId(messageId)
    setDragOverId(null)
    lastDragTargetRef.current = null
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', messageId)
    const row = itemRefs.current.get(messageId)
    if (row) {
      event.dataTransfer.setDragImage(row, 24, row.offsetHeight / 2)
    }
  }

  return (
    <div className="guidance-queue" aria-label={t('chat.guidanceQueue')} ref={menuHostRef}>
      {messages.map((message, index) => {
        const isSubmitting = message.status === 'submitting'
        const visibleContent = stripAttachmentSummary(message.content, message.attachments)
        return (
          <div
            className="guidance-queue__item"
            data-dragging={draggingId === message.id || undefined}
            data-drop-target={dragOverId === message.id || undefined}
            data-status={message.status}
            key={message.id}
            ref={(element) => {
              if (element) {
                itemRefs.current.set(message.id, element)
              } else {
                itemRefs.current.delete(message.id)
              }
            }}
            onDragOver={(event) => {
              if (!draggingId || isSubmitting) return
              event.preventDefault()
              event.dataTransfer.dropEffect = 'move'
              if (draggingId === message.id) {
                setDragOverId(null)
                lastDragTargetRef.current = null
                return
              }
              setDragOverId(message.id)
              if (lastDragTargetRef.current === message.id) return
              lastDragTargetRef.current = message.id
              captureItemPositions()
              onMove(draggingId, message.id)
            }}
            onDrop={(event) => {
              event.preventDefault()
              finishDragging()
            }}
          >
            <button
              aria-label={t('chat.reorderQueuedMessage')}
              className="guidance-queue__drag-handle"
              disabled={isSubmitting}
              draggable={!isSubmitting}
              onDragEnd={finishDragging}
              onDragStart={(event) => startDragging(event, message.id)}
              onKeyDown={(event) => moveWithKeyboard(event, message, index)}
              type="button"
            >
              <GripVertical aria-hidden="true" />
            </button>
            <CornerDownRight className="guidance-queue__leading-icon" aria-hidden="true" />
            <div className="guidance-queue__content">
              <span>{visibleContent || t('chat.attachmentOnlyMessage')}</span>
              {message.attachments.length > 0 && (
                <span className="guidance-queue__attachments">
                  {message.attachments.map((attachment) => attachment.name).join(' · ')}
                </span>
              )}
              {message.status === 'error' && message.error && (
                <span className="guidance-queue__error" role="alert">
                  {message.error}
                </span>
              )}
            </div>
            <div className="guidance-queue__actions">
              <button
                className="guidance-queue__guide"
                disabled={isSubmitting || !guideEnabled}
                onClick={() => onGuide(message)}
                type="button"
              >
                <CornerDownRight aria-hidden="true" />
                <span>
                  {isSubmitting ? t('chat.guidanceSubmitting') : t('chat.guideCurrentRun')}
                </span>
              </button>
              <button
                aria-label={t('chat.deleteQueuedMessage')}
                disabled={isSubmitting}
                onClick={() => onDelete(message.id)}
                type="button"
              >
                <Trash2 aria-hidden="true" />
              </button>
              <div className="guidance-queue__menu-host">
                <button
                  aria-expanded={openMenuId === message.id}
                  aria-haspopup="menu"
                  aria-label={t('chat.queuedMessageMenu')}
                  disabled={isSubmitting}
                  onClick={() =>
                    setOpenMenuId((current) => (current === message.id ? null : message.id))
                  }
                  type="button"
                >
                  <MoreHorizontal aria-hidden="true" />
                </button>
                {openMenuId === message.id && (
                  <div className="guidance-queue__menu" role="menu">
                    <button
                      onClick={() => {
                        setOpenMenuId(null)
                        onEdit(message.id)
                      }}
                      role="menuitem"
                      type="button"
                    >
                      <Pencil aria-hidden="true" />
                      <span>{t('chat.editQueuedMessage')}</span>
                    </button>
                    <button
                      onClick={() => {
                        setOpenMenuId(null)
                        onOpenSideChat(message)
                      }}
                      role="menuitem"
                      type="button"
                    >
                      <PanelRightOpen aria-hidden="true" />
                      <span>{t('chat.openQueuedMessageInSideChat')}</span>
                    </button>
                  </div>
                )}
              </div>
            </div>
          </div>
        )
      })}
    </div>
  )
}
