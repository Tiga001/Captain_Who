import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import type { AppProject } from '../../../config/projectConfig'
import { searchChats, type ChatSearchResult } from '../../../features/search/chatSearchClient'
import type { SidebarConversation } from './leftSidebarTypes'

const CHAT_SEARCH_LIMIT = 100

type SearchStatus = 'idle' | 'searching' | 'complete' | 'error'

interface LeftSidebarSearchDialogLabels {
  chats: string
  empty: string
  inputPlaceholder: string
  noMatches: string
  noProject: string
  searchFailed: string
  searching: string
}

interface LeftSidebarSearchDialogProps {
  conversations: SidebarConversation[]
  labels: LeftSidebarSearchDialogLabels
  onClose: () => void
  onSelectConversation: (conversationId: string, messageId?: string | null) => void
  projects: AppProject[]
}

function renderHighlightedText(value: string, query: string): ReactNode {
  const normalizedQuery = query.trim()
  if (!normalizedQuery) return value

  const valueLower = value.toLocaleLowerCase()
  const queryLower = normalizedQuery.toLocaleLowerCase()
  const parts: ReactNode[] = []
  let cursor = 0
  let matchIndex = valueLower.indexOf(queryLower)

  while (matchIndex >= 0) {
    if (matchIndex > cursor) {
      parts.push(value.slice(cursor, matchIndex))
    }

    const matchEnd = matchIndex + normalizedQuery.length
    parts.push(
      <span className="left-sidebar-search__highlight" key={`${matchIndex}-${matchEnd}`}>
        {value.slice(matchIndex, matchEnd)}
      </span>
    )

    cursor = matchEnd
    matchIndex = valueLower.indexOf(queryLower, cursor)
  }

  if (cursor < value.length) {
    parts.push(value.slice(cursor))
  }

  return parts.length > 0 ? parts : value
}

export function LeftSidebarSearchDialog({
  conversations,
  labels,
  onClose,
  onSelectConversation,
  projects
}: LeftSidebarSearchDialogProps) {
  const [query, setQuery] = useState('')
  const [searchStatus, setSearchStatus] = useState<SearchStatus>('idle')
  const [searchResults, setSearchResults] = useState<ChatSearchResult[]>([])
  const inputRef = useRef<HTMLInputElement>(null)
  const searchRequestIdRef = useRef(0)
  const projectNameById = useMemo(
    () => new Map(projects.map((project) => [project.id, project.name])),
    [projects]
  )
  const recentConversations = useMemo(
    () => [...conversations].sort((a, b) => b.updatedAt - a.updatedAt).slice(0, 9),
    [conversations]
  )
  const normalizedQuery = query.trim()

  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose()
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [onClose])

  useEffect(() => {
    if (!normalizedQuery) {
      searchRequestIdRef.current += 1
      setSearchStatus('idle')
      setSearchResults([])
      return undefined
    }

    const requestId = searchRequestIdRef.current + 1
    searchRequestIdRef.current = requestId
    setSearchStatus('searching')
    setSearchResults([])

    const timeoutId = window.setTimeout(() => {
      void searchChats({ query: normalizedQuery, limit: CHAT_SEARCH_LIMIT })
        .then((results) => {
          if (searchRequestIdRef.current !== requestId) return
          setSearchResults(results)
          setSearchStatus('complete')
        })
        .catch((error) => {
          if (searchRequestIdRef.current !== requestId) return
          console.error('Failed to search chats', error)
          setSearchResults([])
          setSearchStatus('error')
        })
    }, 120)

    return () => window.clearTimeout(timeoutId)
  }, [normalizedQuery])

  const getProjectName = (projectId: string | null | undefined) =>
    projectId ? (projectNameById.get(projectId) ?? labels.noProject) : labels.noProject

  return createPortal(
    <div
      className="left-sidebar-search__backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
    >
      <section
        aria-label={labels.inputPlaceholder}
        aria-modal="true"
        className="left-sidebar-search"
        role="dialog"
      >
        <input
          aria-label={labels.inputPlaceholder}
          className="left-sidebar-search__input"
          onChange={(event) => setQuery(event.target.value)}
          placeholder={labels.inputPlaceholder}
          ref={inputRef}
          spellCheck={false}
          type="text"
          value={query}
        />

        <div className="left-sidebar-search__section-label">{labels.chats}</div>

        {normalizedQuery && searchStatus === 'searching' ? (
          <div className="left-sidebar-search__empty">{labels.searching}</div>
        ) : normalizedQuery && searchStatus === 'error' ? (
          <div className="left-sidebar-search__empty">{labels.searchFailed}</div>
        ) : normalizedQuery && searchStatus === 'complete' && searchResults.length === 0 ? (
          <div className="left-sidebar-search__empty">{labels.noMatches}</div>
        ) : !normalizedQuery && recentConversations.length === 0 ? (
          <div className="left-sidebar-search__empty">{labels.empty}</div>
        ) : (
          <div className="left-sidebar-search__list" role="listbox">
            {normalizedQuery
              ? searchResults.map((result, index) => (
                  <button
                    className="left-sidebar-search__item"
                    data-has-snippet={result.snippet ? 'true' : undefined}
                    key={`${result.conversationId}:${result.messageId ?? 'title'}:${index}`}
                    onClick={() => {
                      onSelectConversation(result.conversationId, result.messageId ?? null)
                      onClose()
                    }}
                    role="option"
                    type="button"
                  >
                    <span className="left-sidebar-search__item-title">
                      {renderHighlightedText(result.title, normalizedQuery)}
                    </span>
                    {result.snippet && (
                      <span className="left-sidebar-search__item-snippet">
                        {renderHighlightedText(result.snippet, normalizedQuery)}
                      </span>
                    )}
                    <span className="left-sidebar-search__item-project">
                      {getProjectName(result.projectId)}
                    </span>
                    <span className="left-sidebar-search__item-index" aria-hidden="true">
                      {index + 1}
                    </span>
                  </button>
                ))
              : recentConversations.map((conversation, index) => {
                  const projectName = getProjectName(conversation.projectId)

                  return (
                    <button
                      className="left-sidebar-search__item"
                      key={conversation.id}
                      onClick={() => {
                        onSelectConversation(conversation.id)
                        onClose()
                      }}
                      role="option"
                      type="button"
                    >
                      <span className="left-sidebar-search__item-title">{conversation.title}</span>
                      <span className="left-sidebar-search__item-project">{projectName}</span>
                      <span className="left-sidebar-search__item-index" aria-hidden="true">
                        {index + 1}
                      </span>
                    </button>
                  )
                })}
          </div>
        )}
      </section>
    </div>,
    document.body
  )
}
