import type { CSSProperties, PointerEvent as ReactPointerEvent } from 'react'
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import {
  Download,
  File,
  FolderOpen,
  MoreHorizontal,
  PauseCircle,
  PlayCircle,
  X
} from 'lucide-react'
import type { BrowserDownloadCenterAction, BrowserDownloadCenterItem } from '@mycopilot/protocol'

import { Tooltip } from '../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { useDismissOnOutsidePointer } from '../../hooks/useDismissOnOutsidePointer'
import { useBrowserDownloadCenter } from './useBrowserDownloadCenter'

interface BrowserDownloadCenterProps {
  isOpen: boolean
  onOpenChange: (open: boolean) => void
}

interface MenuPosition {
  left: number
  top: number
}

interface PanelPosition extends MenuPosition {
  width: number
}

const DOWNLOAD_PANEL_MAX_WIDTH = 420
const DOWNLOAD_PANEL_VIEWPORT_MARGIN = 8
const DOWNLOAD_TOOLTIP_DELAY_MS = 1_000
const ITEM_MENU_WIDTH = 220
const ITEM_MENU_HEIGHT = 170

export function BrowserDownloadCenter({ isOpen, onOpenChange }: BrowserDownloadCenterProps) {
  const { language, t } = useFrontendConfig()
  const { openDirectory, perform, snapshot } = useBrowserDownloadCenter()
  const anchorRef = useRef<HTMLDivElement>(null)
  const [panelPosition, setPanelPosition] = useState<PanelPosition | null>(null)
  const [openMenuId, setOpenMenuId] = useState<string | null>(null)
  const [menuPosition, setMenuPosition] = useState<MenuPosition | null>(null)
  const [busyDownloadId, setBusyDownloadId] = useState<string | null>(null)
  const downloads = snapshot.downloads
  const activeDownloads = downloads.filter((download) => download.canCancel)
  const hasProgressing = activeDownloads.some((download) => download.state === 'progressing')
  const hasPaused = activeDownloads.some(
    (download) => download.state === 'paused' || download.state === 'interrupted'
  )
  const aggregateProgress = aggregateDownloadProgress(activeDownloads)
  const buttonState = hasProgressing ? 'progressing' : hasPaused ? 'paused' : 'idle'
  const buttonStyle = {
    '--browser-download-progress': `${Math.round((aggregateProgress ?? 0) * 360)}deg`
  } as CSSProperties

  const close = useCallback(() => onOpenChange(false), [onOpenChange])
  const ignorePortalContent = useCallback(
    (target: Node) =>
      target instanceof Element &&
      Boolean(target.closest('.browser-download-center__panel, .browser-download-item-menu')),
    []
  )
  useDismissOnOutsidePointer(anchorRef, isOpen, close, ignorePortalContent)

  const updatePanelPosition = useCallback((): void => {
    const anchor = anchorRef.current
    if (!anchor) return
    const anchorBounds = anchor.getBoundingClientRect()
    const browserBounds = anchor.closest('.browser-panel')?.getBoundingClientRect()
    const availableWidth = Math.max(0, window.innerWidth - DOWNLOAD_PANEL_VIEWPORT_MARGIN * 2)
    const width = Math.min(DOWNLOAD_PANEL_MAX_WIDTH, availableWidth)
    const right = Math.min(
      window.innerWidth - DOWNLOAD_PANEL_VIEWPORT_MARGIN,
      browserBounds?.right ?? anchorBounds.right
    )
    const left = Math.max(DOWNLOAD_PANEL_VIEWPORT_MARGIN, right - width)
    setPanelPosition((current) => {
      const next = {
        left: Math.round(left),
        top: Math.round(anchorBounds.bottom + 10),
        width: Math.round(width)
      }
      return current?.left === next.left && current.top === next.top && current.width === next.width
        ? current
        : next
    })
  }, [])

  useLayoutEffect(() => {
    if (!isOpen) {
      setPanelPosition(null)
      return
    }
    updatePanelPosition()
    const anchor = anchorRef.current
    const browserPanel = anchor?.closest('.browser-panel')
    const observer = new ResizeObserver(updatePanelPosition)
    if (anchor) observer.observe(anchor)
    if (browserPanel) observer.observe(browserPanel)
    window.addEventListener('resize', updatePanelPosition)
    window.addEventListener('scroll', updatePanelPosition, true)
    return () => {
      observer.disconnect()
      window.removeEventListener('resize', updatePanelPosition)
      window.removeEventListener('scroll', updatePanelPosition, true)
    }
  }, [isOpen, updatePanelPosition])

  useEffect(() => {
    if (isOpen) return
    setOpenMenuId(null)
    setMenuPosition(null)
  }, [isOpen])

  useEffect(() => {
    if (!openMenuId) return
    const closeItemMenu = (): void => {
      setOpenMenuId(null)
      setMenuPosition(null)
    }
    window.addEventListener('resize', closeItemMenu)
    window.addEventListener('scroll', closeItemMenu, true)
    return () => {
      window.removeEventListener('resize', closeItemMenu)
      window.removeEventListener('scroll', closeItemMenu, true)
    }
  }, [openMenuId])

  const runAction = useCallback(
    async (downloadId: string, action: BrowserDownloadCenterAction): Promise<void> => {
      if (busyDownloadId) return
      setBusyDownloadId(downloadId)
      try {
        await perform(downloadId, action)
      } finally {
        setBusyDownloadId(null)
      }
    },
    [busyDownloadId, perform]
  )

  const openItemMenu = useCallback(
    (event: ReactPointerEvent<HTMLButtonElement>, downloadId: string): void => {
      if (openMenuId === downloadId) {
        setOpenMenuId(null)
        setMenuPosition(null)
        return
      }
      const bounds = event.currentTarget.getBoundingClientRect()
      const left = Math.max(8, Math.min(bounds.right - ITEM_MENU_WIDTH, window.innerWidth - 8))
      const below = bounds.bottom + 6
      const top =
        below + ITEM_MENU_HEIGHT <= window.innerHeight - 8
          ? below
          : Math.max(8, bounds.top - ITEM_MENU_HEIGHT - 6)
      setOpenMenuId(downloadId)
      setMenuPosition({ left, top })
    },
    [openMenuId]
  )

  const menuDownload = useMemo(
    () => downloads.find((download) => download.downloadId === openMenuId) ?? null,
    [downloads, openMenuId]
  )

  return (
    <div className="browser-download-center" ref={anchorRef}>
      <Tooltip
        appearance="inverse"
        content={t('browser.downloadCenter.title')}
        delayMs={DOWNLOAD_TOOLTIP_DELAY_MS}
        preferredPlacement="bottom"
      >
        <button
          aria-expanded={isOpen}
          aria-haspopup="dialog"
          aria-label={t('browser.downloadCenter.title')}
          className="browser-panel__icon-button browser-download-center__trigger"
          data-indeterminate={hasProgressing && aggregateProgress === null ? 'true' : undefined}
          data-state={buttonState}
          onClick={() => onOpenChange(!isOpen)}
          style={buttonStyle}
          type="button"
        >
          <span aria-hidden="true" className="browser-download-center__trigger-ring">
            <Download />
          </span>
        </button>
      </Tooltip>

      {isOpen && panelPosition
        ? createPortal(
            <section
              aria-label={t('browser.downloadCenter.title')}
              className="browser-download-center__panel"
              role="dialog"
              style={panelPosition}
            >
              <header className="browser-download-center__header">
                <h2>{t('browser.downloadCenter.title')}</h2>
                <Tooltip
                  appearance="inverse"
                  content={t('browser.downloadCenter.openFolder')}
                  delayMs={DOWNLOAD_TOOLTIP_DELAY_MS}
                >
                  <button
                    aria-label={t('browser.downloadCenter.openFolder')}
                    className="browser-download-center__header-button"
                    onClick={() => void openDirectory()}
                    type="button"
                  >
                    <FolderOpen aria-hidden="true" />
                  </button>
                </Tooltip>
              </header>

              {downloads.length === 0 ? (
                <div className="browser-download-center__empty">
                  <Download aria-hidden="true" />
                  <span>{t('browser.downloadCenter.empty')}</span>
                </div>
              ) : (
                <div className="browser-download-center__list">
                  {downloads.map((download) => {
                    const progress = itemProgress(download)
                    return (
                      <article
                        className="browser-download-center__item"
                        data-state={download.state}
                        key={download.downloadId}
                      >
                        <span aria-hidden="true" className="browser-download-center__file-icon">
                          <File />
                        </span>
                        <div className="browser-download-center__item-copy">
                          <strong title={download.displayName}>{download.displayName}</strong>
                          <span>
                            {formatDownloadSummary(download, language, {
                              completed: t('browser.downloadCenter.completed'),
                              cancelled: t('browser.downloadCenter.cancelled'),
                              hourUnit: t('browser.downloadCenter.hourUnit'),
                              interrupted: t('browser.downloadCenter.interrupted'),
                              lessThanMinute: t('browser.downloadCenter.lessThanMinute'),
                              minuteUnit: t('browser.downloadCenter.minuteUnit'),
                              paused: t('browser.downloadCenter.paused')
                            })}
                          </span>
                          {(download.state === 'progressing' || download.state === 'paused') && (
                            <span
                              aria-label={`${Math.round(progress * 100)}%`}
                              aria-valuemax={100}
                              aria-valuemin={0}
                              aria-valuenow={Math.round(progress * 100)}
                              className="browser-download-center__progress"
                              role="progressbar"
                            >
                              <span style={{ width: `${Math.round(progress * 100)}%` }} />
                            </span>
                          )}
                        </div>
                        <div className="browser-download-center__item-actions">
                          {download.canPause && (
                            <Tooltip
                              appearance="inverse"
                              content={t('browser.downloadCenter.pause')}
                              delayMs={DOWNLOAD_TOOLTIP_DELAY_MS}
                            >
                              <button
                                aria-label={t('browser.downloadCenter.pause')}
                                disabled={busyDownloadId === download.downloadId}
                                onClick={() => void runAction(download.downloadId, 'pause')}
                                type="button"
                              >
                                <PauseCircle aria-hidden="true" />
                              </button>
                            </Tooltip>
                          )}
                          {download.canResume && (
                            <Tooltip
                              appearance="inverse"
                              content={t('browser.downloadCenter.resume')}
                              delayMs={DOWNLOAD_TOOLTIP_DELAY_MS}
                            >
                              <button
                                aria-label={t('browser.downloadCenter.resume')}
                                disabled={busyDownloadId === download.downloadId}
                                onClick={() => void runAction(download.downloadId, 'resume')}
                                type="button"
                              >
                                <PlayCircle aria-hidden="true" />
                              </button>
                            </Tooltip>
                          )}
                          {download.canCancel && (
                            <Tooltip
                              appearance="inverse"
                              content={t('browser.downloadCenter.stop')}
                              delayMs={DOWNLOAD_TOOLTIP_DELAY_MS}
                            >
                              <button
                                aria-label={t('browser.downloadCenter.stop')}
                                disabled={busyDownloadId === download.downloadId}
                                onClick={() => void runAction(download.downloadId, 'cancel')}
                                type="button"
                              >
                                <X aria-hidden="true" />
                              </button>
                            </Tooltip>
                          )}
                          {!download.canCancel && (
                            <button
                              aria-expanded={openMenuId === download.downloadId}
                              aria-haspopup="menu"
                              aria-label={t('browser.downloadCenter.moreActions')}
                              onPointerDown={(event) => openItemMenu(event, download.downloadId)}
                              type="button"
                            >
                              <MoreHorizontal aria-hidden="true" />
                            </button>
                          )}
                        </div>
                      </article>
                    )
                  })}
                </div>
              )}
            </section>,
            document.body
          )
        : null}

      {menuDownload && menuPosition
        ? createPortal(
            <div
              className="browser-download-item-menu"
              role="menu"
              style={{ left: menuPosition.left, top: menuPosition.top }}
            >
              <button
                disabled={!menuDownload.canReveal}
                onClick={() => {
                  void runAction(menuDownload.downloadId, 'reveal')
                  setOpenMenuId(null)
                }}
                role="menuitem"
                type="button"
              >
                {t('browser.downloadCenter.reveal')}
              </button>
              <button
                disabled={!menuDownload.canCopyUrl}
                onClick={() => {
                  void runAction(menuDownload.downloadId, 'copy_url')
                  setOpenMenuId(null)
                }}
                role="menuitem"
                type="button"
              >
                {t('browser.downloadCenter.copyUrl')}
              </button>
              <button
                disabled={!menuDownload.canCopyPath}
                onClick={() => {
                  void runAction(menuDownload.downloadId, 'copy_path')
                  setOpenMenuId(null)
                }}
                role="menuitem"
                type="button"
              >
                {t('browser.downloadCenter.copyPath')}
              </button>
              <hr />
              <button
                onClick={() => {
                  void runAction(menuDownload.downloadId, 'remove')
                  setOpenMenuId(null)
                }}
                role="menuitem"
                type="button"
              >
                {t('browser.downloadCenter.remove')}
              </button>
            </div>,
            document.body
          )
        : null}
    </div>
  )
}

function aggregateDownloadProgress(downloads: readonly BrowserDownloadCenterItem[]): number | null {
  const known = downloads.filter((download) => download.totalBytes > 0)
  if (known.length === 0) return null
  const total = known.reduce((sum, download) => sum + download.totalBytes, 0)
  const received = known.reduce(
    (sum, download) => sum + Math.min(download.receivedBytes, download.totalBytes),
    0
  )
  return total > 0 ? Math.min(1, received / total) : null
}

function itemProgress(download: BrowserDownloadCenterItem): number {
  if (download.totalBytes <= 0) return 0
  return Math.min(1, download.receivedBytes / download.totalBytes)
}

interface DownloadSummaryLabels {
  cancelled: string
  completed: string
  hourUnit: string
  interrupted: string
  lessThanMinute: string
  minuteUnit: string
  paused: string
}

function formatDownloadSummary(
  download: BrowserDownloadCenterItem,
  locale: string,
  labels: DownloadSummaryLabels
): string {
  const transferred = formatTransferredBytes(download, locale)
  if (download.state === 'progressing') {
    const parts = [
      download.bytesPerSecond > 0 ? `${formatBytes(download.bytesPerSecond, locale)}/s` : null,
      transferred,
      formatRemainingTime(download, labels)
    ].filter((part): part is string => Boolean(part))
    return parts.join(' · ')
  }
  if (download.state === 'paused') return [labels.paused, transferred].join(' · ')
  if (download.state === 'interrupted' && download.canResume) {
    return [labels.interrupted, transferred].join(' · ')
  }
  const status =
    download.state === 'completed'
      ? labels.completed
      : download.state === 'cancelled'
        ? labels.cancelled
        : labels.interrupted
  return `${status} · ${formatClockTime(download.updatedAt, locale)}`
}

function formatTransferredBytes(download: BrowserDownloadCenterItem, locale: string): string {
  const received = formatBytes(download.receivedBytes, locale)
  return download.totalBytes > 0
    ? `${received} / ${formatBytes(download.totalBytes, locale)}`
    : received
}

function formatRemainingTime(
  download: BrowserDownloadCenterItem,
  labels: DownloadSummaryLabels
): string | null {
  if (
    download.bytesPerSecond <= 0 ||
    download.totalBytes <= 0 ||
    download.receivedBytes >= download.totalBytes
  ) {
    return null
  }
  const seconds = (download.totalBytes - download.receivedBytes) / download.bytesPerSecond
  if (seconds < 60) return labels.lessThanMinute
  if (seconds < 3_600) return `${Math.ceil(seconds / 60)} ${labels.minuteUnit}`
  return `${Math.ceil(seconds / 3_600)} ${labels.hourUnit}`
}

function formatBytes(bytes: number, locale: string): string {
  if (bytes < 1_024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB', 'TB'] as const
  let value = bytes / 1_024
  let unitIndex = 0
  while (value >= 1_024 && unitIndex < units.length - 1) {
    value /= 1_024
    unitIndex += 1
  }
  return `${new Intl.NumberFormat(locale, {
    maximumFractionDigits: value < 10 ? 1 : 0,
    minimumFractionDigits: value < 10 ? 1 : 0
  }).format(value)} ${units[unitIndex]}`
}

function formatClockTime(timestamp: number, locale: string): string {
  return new Intl.DateTimeFormat(locale, {
    hour: '2-digit',
    minute: '2-digit'
  }).format(new Date(timestamp))
}
