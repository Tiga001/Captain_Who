// Renderer UI.
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import type { CSSProperties } from 'react'
import { createPortal } from 'react-dom'
import { useFrontendConfig } from '../../../../config/FrontendConfigProvider'
import type { ChatWebSearchSource } from '../../chatTypes'
import { compareWebSearchSourcesByRelevance } from '../../agentWebSearch'
import { openExternalUrl } from '../../../../lib/externalLinks'
import { hostClient } from '../../../../host/hostClient'

const ASSISTANT_SOURCES_POPOVER_GAP = 10
const ASSISTANT_SOURCES_VIEWPORT_MARGIN = 16
const ASSISTANT_SOURCES_MAX_WIDTH = 620
const ASSISTANT_SOURCES_HEADER_HEIGHT = 33
const ASSISTANT_SOURCES_ROW_HEIGHT = 34
const ASSISTANT_SOURCES_MAX_VISIBLE_ITEMS = 5
const faviconCache = new Map<string, Promise<string | null>>()

function faviconCacheKey(source: ChatWebSearchSource) {
  return `${source.url}\n${source.faviconUrl ?? ''}`
}

function resolveSourceFavicon(source: ChatWebSearchSource): Promise<string | null> {
  const key = faviconCacheKey(source)
  const cached = faviconCache.get(key)
  if (cached) return cached

  const request = hostClient.resources
    .resolveFavicon({
      pageUrl: source.url,
      faviconUrl: source.faviconUrl ?? null
    })
    .then((response) => response.url)
    .catch(() => null)

  faviconCache.set(key, request)
  return request
}

function getDomainInitial(domain: string) {
  return (domain.replace(/^www\./, '').match(/[a-z0-9]/i)?.[0] ?? 'W').toUpperCase()
}

function openSourceUrl(url: string) {
  void openExternalUrl(url).catch((error) => {
    console.error('Failed to open external URL', error)
  })
}

export function SourceBadge({ source }: { source: ChatWebSearchSource }) {
  const [imageFailed, setImageFailed] = useState(false)
  const [faviconUrl, setFaviconUrl] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setImageFailed(false)
    setFaviconUrl(null)

    void resolveSourceFavicon(source).then((url) => {
      if (!cancelled) {
        setFaviconUrl(url)
      }
    })

    return () => {
      cancelled = true
    }
  }, [source.url, source.faviconUrl])

  if (faviconUrl && !imageFailed) {
    return (
      <span className="web-source-badge web-source-badge--image" aria-hidden="true">
        <img alt="" src={faviconUrl} onError={() => setImageFailed(true)} />
      </span>
    )
  }

  return (
    <span className="web-source-badge" aria-hidden="true">
      {getDomainInitial(source.domain)}
    </span>
  )
}

export function WebSearchSourcesList({
  mode = 'compact',
  sources
}: {
  mode?: 'compact' | 'rich'
  sources: ChatWebSearchSource[]
}) {
  const { t } = useFrontendConfig()

  if (sources.length === 0) {
    return <p className="web-search-activity__empty">{t('agent.web.noSources')}</p>
  }

  const orderedSources = [...sources].sort(compareWebSearchSourcesByRelevance)

  return (
    <div className="web-search-sources-list">
      {orderedSources.map((source) => (
        <button
          className="web-search-source"
          key={source.id}
          onClick={() => openSourceUrl(source.url)}
          type="button"
        >
          <SourceBadge source={source} />
          <span className="web-search-source__content">
            <span className="web-search-source__url">{source.displayUrl}</span>
            {mode === 'rich' && (
              <>
                <span className="web-search-source__title">{source.title}</span>
                {source.snippet && (
                  <span className="web-search-source__snippet">{source.snippet}</span>
                )}
                {source.publishedDate && (
                  <span className="web-search-source__meta">
                    <span>{source.publishedDate}</span>
                  </span>
                )}
              </>
            )}
          </span>
        </button>
      ))}
    </div>
  )
}

export function AssistantSources({ sources }: { sources: ChatWebSearchSource[] }) {
  const { t } = useFrontendConfig()
  const [isOpen, setOpen] = useState(false)
  const [popoverStyle, setPopoverStyle] = useState<CSSProperties | null>(null)
  const rootRef = useRef<HTMLDivElement>(null)
  const buttonRef = useRef<HTMLButtonElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const visibleItemCount = Math.min(sources.length, ASSISTANT_SOURCES_MAX_VISIBLE_ITEMS)
  const sourcesListMaxHeight = visibleItemCount * ASSISTANT_SOURCES_ROW_HEIGHT

  const updatePopoverPosition = useCallback(() => {
    const button = buttonRef.current
    if (!button) return

    const rect = button.getBoundingClientRect()
    const viewportWidth = window.innerWidth
    const viewportHeight = window.innerHeight
    const width = Math.min(
      ASSISTANT_SOURCES_MAX_WIDTH,
      Math.max(280, viewportWidth - ASSISTANT_SOURCES_VIEWPORT_MARGIN * 2)
    )
    const estimatedHeight =
      ASSISTANT_SOURCES_HEADER_HEIGHT + sourcesListMaxHeight + ASSISTANT_SOURCES_VIEWPORT_MARGIN * 2
    const measuredHeight = popoverRef.current?.offsetHeight ?? estimatedHeight
    const spaceAbove = rect.top - ASSISTANT_SOURCES_VIEWPORT_MARGIN
    const spaceBelow = viewportHeight - rect.bottom - ASSISTANT_SOURCES_VIEWPORT_MARGIN
    const shouldPlaceBelow =
      spaceBelow >= measuredHeight || (spaceBelow >= spaceAbove && spaceAbove < measuredHeight)
    const rawTop = shouldPlaceBelow
      ? rect.bottom + ASSISTANT_SOURCES_POPOVER_GAP
      : rect.top - measuredHeight - ASSISTANT_SOURCES_POPOVER_GAP
    const top = Math.min(
      Math.max(rawTop, ASSISTANT_SOURCES_VIEWPORT_MARGIN),
      Math.max(
        ASSISTANT_SOURCES_VIEWPORT_MARGIN,
        viewportHeight - measuredHeight - ASSISTANT_SOURCES_VIEWPORT_MARGIN
      )
    )
    const left = Math.min(
      Math.max(rect.left, ASSISTANT_SOURCES_VIEWPORT_MARGIN),
      Math.max(
        ASSISTANT_SOURCES_VIEWPORT_MARGIN,
        viewportWidth - width - ASSISTANT_SOURCES_VIEWPORT_MARGIN
      )
    )

    setPopoverStyle({
      left,
      top,
      width,
      '--assistant-sources-list-max-height': `${sourcesListMaxHeight}px`
    } as CSSProperties)
  }, [sourcesListMaxHeight])

  useEffect(() => {
    if (!isOpen) return undefined

    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target
      if (!(target instanceof Node)) return
      if (rootRef.current?.contains(target)) return
      if (popoverRef.current?.contains(target)) return
      setOpen(false)
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false)
      }
    }

    document.addEventListener('pointerdown', handlePointerDown)
    document.addEventListener('keydown', handleKeyDown)
    return () => {
      document.removeEventListener('pointerdown', handlePointerDown)
      document.removeEventListener('keydown', handleKeyDown)
    }
  }, [isOpen])

  useLayoutEffect(() => {
    if (!isOpen) {
      setPopoverStyle(null)
      return undefined
    }

    updatePopoverPosition()
    const animationFrame = window.requestAnimationFrame(updatePopoverPosition)
    window.addEventListener('resize', updatePopoverPosition)
    document.addEventListener('scroll', updatePopoverPosition, true)
    return () => {
      window.cancelAnimationFrame(animationFrame)
      window.removeEventListener('resize', updatePopoverPosition)
      document.removeEventListener('scroll', updatePopoverPosition, true)
    }
  }, [isOpen, updatePopoverPosition])

  if (sources.length === 0) return null

  return (
    <div className="assistant-sources" ref={rootRef}>
      <button
        ref={buttonRef}
        aria-expanded={isOpen}
        className="assistant-sources__button"
        onClick={() => setOpen((current) => !current)}
        type="button"
      >
        <span className="assistant-sources__badges" aria-hidden="true">
          {sources.slice(0, 3).map((source) => (
            <SourceBadge key={source.id} source={source} />
          ))}
        </span>
        <span>{t('agent.web.sources')}</span>
      </button>
      {isOpen &&
        createPortal(
          <div
            className="assistant-sources__popover"
            ref={popoverRef}
            role="dialog"
            aria-label={t('agent.web.sourcesAria')}
            style={popoverStyle ?? undefined}
          >
            <strong>{t('agent.web.sources')}</strong>
            <WebSearchSourcesList sources={sources} />
          </div>,
          document.body
        )}
    </div>
  )
}
