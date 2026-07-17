import { useCallback, useEffect, useRef, useState } from 'react'
import type { KeyboardEvent, ReactNode } from 'react'
import type { PDFDocumentLoadingTask, PDFDocumentProxy, PDFPageProxy, RenderTask } from 'pdfjs-dist'
import type { WorkspacePdfFileContent } from '@mycopilot/protocol'
import { AlertCircle, ChevronLeft, ChevronRight, LoaderCircle } from 'lucide-react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation } from '../../config/translationFormat'
import { createWorkspacePdfLoadingTask } from './workspacePdfRuntime'

const MAX_OUTPUT_SCALE = 2
const RESIZE_SETTLE_MS = 90

type PdfFailureKind = 'invalid' | 'password' | 'render' | 'unknown'

type PdfDocumentState =
  | { status: 'loading' }
  | { failure: PdfFailureKind; status: 'error' }
  | { document: PDFDocumentProxy; numPages: number; status: 'ready' }

type PdfRenderState = 'idle' | 'rendering' | 'ready' | 'error'

interface ViewportSize {
  height: number
  width: number
}

interface WorkspacePdfPreviewProps {
  content: WorkspacePdfFileContent
  initialPage: number
  onPageChange: (page: number) => void
  onRetry: () => void
  path: string
}

export function WorkspacePdfPreview({
  content,
  initialPage,
  onPageChange,
  onRetry,
  path
}: WorkspacePdfPreviewProps): ReactNode {
  const { t } = useFrontendConfig()
  const [documentState, setDocumentState] = useState<PdfDocumentState>({ status: 'loading' })
  const [pageNumber, setPageNumber] = useState(() => normalizePageNumber(initialPage, 1))
  const [renderedPageNumber, setRenderedPageNumber] = useState<number | null>(null)
  const [renderState, setRenderState] = useState<PdfRenderState>('idle')
  const [viewportSize, setViewportSize] = useState<ViewportSize>({ height: 0, width: 0 })
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const viewportRef = useRef<HTMLDivElement>(null)
  const renderSequenceRef = useRef(0)
  const initialPageRef = useRef(initialPage)
  const onPageChangeRef = useRef(onPageChange)

  useEffect(() => {
    initialPageRef.current = initialPage
  }, [initialPage])

  useEffect(() => {
    onPageChangeRef.current = onPageChange
  }, [onPageChange])

  useEffect(() => {
    let disposed = false
    let loadingTask: PDFDocumentLoadingTask | null = null
    let document: PDFDocumentProxy | null = null

    setDocumentState({ status: 'loading' })
    setRenderState('idle')
    setRenderedPageNumber(null)

    void (async () => {
      try {
        loadingTask = await createWorkspacePdfLoadingTask(content.data)
        if (disposed) {
          await loadingTask.destroy().catch(() => undefined)
          return
        }

        document = await loadingTask.promise
        if (disposed) {
          await document.destroy().catch(() => undefined)
          return
        }
        if (!Number.isSafeInteger(document.numPages) || document.numPages < 1) {
          await document.destroy().catch(() => undefined)
          document = null
          setDocumentState({ failure: 'invalid', status: 'error' })
          return
        }

        const requestedPage = initialPageRef.current
        const normalizedPage = normalizePageNumber(requestedPage, document.numPages)
        setPageNumber(normalizedPage)
        if (normalizedPage !== requestedPage) onPageChangeRef.current(normalizedPage)
        setDocumentState({ document, numPages: document.numPages, status: 'ready' })
      } catch (error) {
        if (!disposed) {
          setDocumentState({ failure: classifyPdfFailure(error), status: 'error' })
        }
      }
    })()

    return () => {
      disposed = true
      renderSequenceRef.current += 1
      if (document) {
        void document.destroy().catch(() => undefined)
      } else if (loadingTask) {
        void loadingTask.destroy().catch(() => undefined)
      }
    }
  }, [content.data])

  useEffect(() => {
    const viewport = viewportRef.current
    if (!viewport) return

    let resizeTimer: number | null = null
    let hasMeasured = false
    const commitSize = (width: number, height: number): void => {
      const next = {
        height: Math.max(0, Math.floor(height)),
        width: Math.max(0, Math.floor(width))
      }
      setViewportSize((current) =>
        current.width === next.width && current.height === next.height ? current : next
      )
    }
    const scheduleSize = (width: number, height: number): void => {
      if (!hasMeasured) {
        hasMeasured = true
        commitSize(width, height)
        return
      }
      if (resizeTimer !== null) window.clearTimeout(resizeTimer)
      resizeTimer = window.setTimeout(() => commitSize(width, height), RESIZE_SETTLE_MS)
    }

    const observer = new ResizeObserver((entries) => {
      const entry = entries.at(-1)
      if (entry) scheduleSize(entry.contentRect.width, entry.contentRect.height)
    })
    observer.observe(viewport)

    return () => {
      observer.disconnect()
      if (resizeTimer !== null) window.clearTimeout(resizeTimer)
    }
  }, [])

  useEffect(() => {
    if (documentState.status !== 'ready' || viewportSize.width <= 0 || viewportSize.height <= 0) {
      return
    }

    const sequence = renderSequenceRef.current + 1
    renderSequenceRef.current = sequence
    let disposed = false
    let page: PDFPageProxy | null = null
    let renderTask: RenderTask | null = null

    setRenderState('rendering')
    void (async () => {
      try {
        page = await documentState.document.getPage(pageNumber)
        if (disposed || renderSequenceRef.current !== sequence) return

        const baseViewport = page.getViewport({ scale: 1 })
        const availableWidth = Math.max(1, viewportSize.width)
        const availableHeight = Math.max(1, viewportSize.height)
        const cssScale = Math.min(
          availableWidth / baseViewport.width,
          availableHeight / baseViewport.height
        )
        if (!Number.isFinite(cssScale) || cssScale <= 0) throw new Error('Invalid PDF page size')

        const outputScale = Math.min(Math.max(window.devicePixelRatio || 1, 1), MAX_OUTPUT_SCALE)
        const renderViewport = page.getViewport({ scale: cssScale * outputScale })
        const renderCanvas = document.createElement('canvas')
        renderCanvas.width = Math.max(1, Math.ceil(renderViewport.width))
        renderCanvas.height = Math.max(1, Math.ceil(renderViewport.height))

        renderTask = page.render({ canvas: renderCanvas, viewport: renderViewport })
        await renderTask.promise
        if (disposed || renderSequenceRef.current !== sequence) return

        const visibleCanvas = canvasRef.current
        if (!visibleCanvas) return
        const context = visibleCanvas.getContext('2d', { alpha: false })
        if (!context) throw new Error('Canvas rendering is unavailable')

        visibleCanvas.width = renderCanvas.width
        visibleCanvas.height = renderCanvas.height
        visibleCanvas.style.width = `${Math.max(1, renderViewport.width / outputScale)}px`
        visibleCanvas.style.height = `${Math.max(1, renderViewport.height / outputScale)}px`
        context.drawImage(renderCanvas, 0, 0)
        setRenderedPageNumber(pageNumber)
        setRenderState('ready')
      } catch (error) {
        if (!disposed && renderSequenceRef.current === sequence && !isRenderCancellation(error)) {
          setRenderState('error')
        }
      } finally {
        page?.cleanup()
      }
    })()

    return () => {
      disposed = true
      renderTask?.cancel()
      page?.cleanup()
    }
  }, [documentState, pageNumber, viewportSize])

  const goToPage = useCallback(
    (requestedPage: number) => {
      if (documentState.status !== 'ready') return
      const nextPage = normalizePageNumber(requestedPage, documentState.numPages)
      if (nextPage === pageNumber) return
      setPageNumber(nextPage)
      onPageChangeRef.current(nextPage)
    },
    [documentState, pageNumber]
  )

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    if (event.altKey || event.ctrlKey || event.metaKey) return
    let nextPage: number | null = null
    if (event.key === 'ArrowLeft' || event.key === 'PageUp') nextPage = pageNumber - 1
    if (event.key === 'ArrowRight' || event.key === 'PageDown' || event.key === ' ') {
      nextPage = pageNumber + 1
    }
    if (event.key === 'Home') nextPage = 1
    if (event.key === 'End' && documentState.status === 'ready') {
      nextPage = documentState.numPages
    }
    if (nextPage !== null) {
      event.preventDefault()
      goToPage(nextPage)
    }
  }

  if (documentState.status === 'error') {
    return (
      <PdfPreviewMessage
        actionLabel={t('files.retry')}
        description={pdfFailureDescription(documentState.failure, t)}
        onAction={onRetry}
        title={t('files.pdf.errorTitle')}
      />
    )
  }

  const numPages = documentState.status === 'ready' ? documentState.numPages : 0
  const isPagePending = renderState === 'rendering' && renderedPageNumber !== pageNumber

  return (
    <div
      className="files-panel__pdf-preview"
      aria-label={formatTranslation(t, 'files.pdf.previewLabel', { file: path })}
      aria-busy={documentState.status === 'loading' || isPagePending}
      onKeyDown={handleKeyDown}
      tabIndex={0}
    >
      <div className="files-panel__pdf-viewport" ref={viewportRef}>
        <canvas
          className="files-panel__pdf-page"
          aria-label={formatTranslation(t, 'files.pdf.pageLabel', { page: pageNumber })}
          ref={canvasRef}
        />
      </div>

      {(documentState.status === 'loading' || isPagePending || renderedPageNumber === null) && (
        <div className="files-panel__pdf-loading" role="status">
          <LoaderCircle className="files-panel__spinner" aria-hidden="true" />
          <span>{t('files.preview.loading')}</span>
        </div>
      )}

      {renderState === 'error' && (
        <PdfPreviewMessage
          actionLabel={t('files.retry')}
          description={pdfFailureDescription('render', t)}
          onAction={onRetry}
          title={t('files.pdf.errorTitle')}
        />
      )}

      {documentState.status === 'ready' && renderState !== 'error' && (
        <div className="files-panel__pdf-pager" aria-label={t('files.pdf.pageNavigation')}>
          <button
            type="button"
            aria-label={t('files.pdf.previousPage')}
            disabled={pageNumber <= 1}
            onClick={() => goToPage(pageNumber - 1)}
            title={t('files.pdf.previousPage')}
          >
            <ChevronLeft aria-hidden="true" />
          </button>
          <span
            aria-live="polite"
            aria-label={formatTranslation(t, 'files.pdf.pageStatus', {
              current: pageNumber,
              total: numPages
            })}
          >
            {pageNumber}/{numPages}
          </span>
          <button
            type="button"
            aria-label={t('files.pdf.nextPage')}
            disabled={pageNumber >= numPages}
            onClick={() => goToPage(pageNumber + 1)}
            title={t('files.pdf.nextPage')}
          >
            <ChevronRight aria-hidden="true" />
          </button>
        </div>
      )}
    </div>
  )
}

interface PdfPreviewMessageProps {
  actionLabel: string
  description: string
  onAction: () => void
  title: string
}

function PdfPreviewMessage({
  actionLabel,
  description,
  onAction,
  title
}: PdfPreviewMessageProps): ReactNode {
  return (
    <div className="files-panel__center-state files-panel__pdf-error">
      <div className="files-panel__center-icon">
        <AlertCircle aria-hidden="true" />
      </div>
      <h2>{title}</h2>
      <p>{description}</p>
      <button type="button" onClick={onAction}>
        {actionLabel}
      </button>
    </div>
  )
}

function normalizePageNumber(page: number, numPages: number): number {
  if (!Number.isSafeInteger(page)) return 1
  return Math.min(Math.max(page, 1), Math.max(numPages, 1))
}

function classifyPdfFailure(error: unknown): PdfFailureKind {
  const name = error instanceof Error ? error.name : ''
  if (name === 'PasswordException') return 'password'
  if (name === 'InvalidPDFException' || name === 'MissingPDFException') return 'invalid'
  return 'unknown'
}

function isRenderCancellation(error: unknown): boolean {
  return error instanceof Error && error.name === 'RenderingCancelledException'
}

function pdfFailureDescription(
  failure: PdfFailureKind,
  t: ReturnType<typeof useFrontendConfig>['t']
): string {
  if (failure === 'password') return t('files.pdf.passwordProtected')
  if (failure === 'invalid') return t('files.pdf.invalid')
  if (failure === 'render') return t('files.pdf.renderError')
  return t('files.pdf.loadError')
}
