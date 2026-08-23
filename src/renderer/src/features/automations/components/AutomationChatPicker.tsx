import { Check, ChevronDown, MessageCircle, Search, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { KeyboardEvent } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import type { AppProject } from '../../../config/projectConfig'
import type { ChatConversation } from '../../chat/chatTypes'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'
import { formatAbsoluteDateTime } from '../automationPresentation'

interface AutomationChatPickerProps {
  conversations: readonly ChatConversation[]
  disabled?: boolean
  fallbackTitle?: string | null
  onChange: (conversationId: string | null) => void
  projects: readonly AppProject[]
  value: string | null
}

interface ChatPickerRow {
  conversation: ChatConversation
  projectName: string
}

export function AutomationChatPicker({
  conversations,
  disabled = false,
  fallbackTitle,
  onChange,
  projects,
  value
}: AutomationChatPickerProps) {
  const { language, t } = useFrontendConfig()
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const rootRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([])
  const close = useCallback(() => {
    setOpen(false)
    setQuery('')
  }, [])
  useDismissOnOutsidePointer(rootRef, open, close)
  useEffect(() => {
    if (disabled) close()
  }, [close, disabled])

  const eligible = useMemo(
    () =>
      conversations.filter(
        (conversation) => !conversation.archivedAt && conversation.pendingArchivedAt === undefined
      ),
    [conversations]
  )
  const selected = eligible.find((conversation) => conversation.id === value) ?? null
  const rows = useMemo<ChatPickerRow[]>(() => {
    const normalized = query.trim().toLocaleLowerCase(language)
    const projectNames = new Map(projects.map((project) => [project.id, project.name]))
    return eligible
      .filter((conversation) =>
        normalized ? conversation.title.toLocaleLowerCase(language).includes(normalized) : true
      )
      .map((conversation) => ({
        conversation,
        projectName:
          (conversation.projectId ? projectNames.get(conversation.projectId) : null) ??
          t('automation.noProject')
      }))
      .sort((left, right) => {
        const projectCompare = left.projectName.localeCompare(right.projectName, language)
        return projectCompare || right.conversation.updatedAt - left.conversation.updatedAt
      })
  }, [eligible, language, projects, query, t])

  const openPicker = () => {
    setOpen(true)
    window.requestAnimationFrame(() => searchRef.current?.focus())
  }

  const select = (conversationId: string | null) => {
    if (disabled) return
    onChange(conversationId)
    close()
  }

  const focusOption = (index: number) => {
    const normalized = (index + optionRefs.current.length) % optionRefs.current.length
    optionRefs.current[normalized]?.focus()
  }

  const handleOptionKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      focusOption(index + (event.key === 'ArrowDown' ? 1 : -1))
    } else if (event.key === 'Escape') {
      event.preventDefault()
      event.stopPropagation()
      close()
    }
  }

  const displayedRows = rows.map((row, index) => ({
    ...row,
    optionIndex: index + 1,
    showHeading: index === 0 || rows[index - 1]?.projectName !== row.projectName
  }))

  return (
    <div className="automation-chat-picker" ref={rootRef} data-open={open || undefined}>
      <button
        type="button"
        className="automation-select__trigger"
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={`${t('automation.chat')}: ${selected?.title ?? fallbackTitle ?? t('automation.chooseChat')}`}
        disabled={disabled}
        onClick={() => (open ? close() : openPicker())}
      >
        <MessageCircle aria-hidden="true" />
        <span>{selected?.title ?? fallbackTitle ?? t('automation.chooseChat')}</span>
        <ChevronDown aria-hidden="true" />
      </button>

      {open && (
        <div
          className="automation-chat-picker__popover"
          role="dialog"
          aria-label={t('automation.chooseChat')}
        >
          <div className="automation-chat-picker__search">
            <Search aria-hidden="true" />
            <input
              ref={searchRef}
              value={query}
              type="search"
              placeholder={t('automation.searchChats')}
              aria-label={t('automation.searchChats')}
              onChange={(event) => setQuery(event.currentTarget.value)}
              onKeyDown={(event) => {
                if (event.key === 'ArrowDown') {
                  event.preventDefault()
                  focusOption(0)
                } else if (event.key === 'Escape') {
                  event.preventDefault()
                  event.stopPropagation()
                  close()
                }
              }}
            />
            {query && (
              <button
                type="button"
                aria-label={t('automation.clearSearch')}
                onClick={() => setQuery('')}
              >
                <X aria-hidden="true" />
              </button>
            )}
          </div>
          <div
            className="automation-chat-picker__list"
            role="listbox"
            aria-label={t('automation.chat')}
          >
            <button
              ref={(node) => {
                optionRefs.current[0] = node
              }}
              type="button"
              role="option"
              aria-selected={value === null}
              className="automation-chat-picker__option"
              data-selected={value === null || undefined}
              onClick={() => select(null)}
              onKeyDown={(event) => handleOptionKeyDown(event, 0)}
            >
              <MessageCircle aria-hidden="true" />
              <span>{t('automation.destinationNewChat')}</span>
              {value === null && <Check aria-hidden="true" />}
            </button>
            {rows.length === 0 && (
              <p className="automation-chat-picker__empty">{t('automation.noChats')}</p>
            )}
            {displayedRows.map(({ conversation, optionIndex, projectName, showHeading }) => {
              return (
                <div className="automation-chat-picker__group-row" key={conversation.id}>
                  {showHeading && <p className="automation-chat-picker__group">{projectName}</p>}
                  <button
                    ref={(node) => {
                      optionRefs.current[optionIndex] = node
                    }}
                    type="button"
                    role="option"
                    aria-selected={conversation.id === value}
                    className="automation-chat-picker__option"
                    data-selected={conversation.id === value || undefined}
                    onClick={() => select(conversation.id)}
                    onKeyDown={(event) => handleOptionKeyDown(event, optionIndex)}
                  >
                    <MessageCircle aria-hidden="true" />
                    <span>{conversation.title}</span>
                    <time dateTime={new Date(conversation.updatedAt).toISOString()}>
                      {formatAbsoluteDateTime(conversation.updatedAt, language)}
                    </time>
                    {conversation.id === value && <Check aria-hidden="true" />}
                  </button>
                </div>
              )
            })}
          </div>
        </div>
      )}
    </div>
  )
}
