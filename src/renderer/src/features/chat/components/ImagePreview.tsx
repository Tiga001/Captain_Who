// Renderer chat image preview overlay.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type WheelEvent as ReactWheelEvent
} from 'react'
import { Copy, Download, Minus, Plus, X } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { hostClient } from '../../../host/hostClient'
import './ImagePreview.css'

interface ImagePreviewInput {
  alt?: string
  fileName?: string
  src: string
}

interface ImagePreviewContextValue {
  copyImageToClipboard: (src: string) => Promise<boolean>
  openImagePreview: (input: ImagePreviewInput) => void
  showImagePreviewNotice: (message: string) => void
}

type CopyState = 'idle' | 'copying' | 'copied' | 'failed'

interface ContextMenuState {
  x: number
  y: number
}

const MIN_ZOOM = 0.25
const MAX_ZOOM = 4
const ZOOM_STEP = 0.25

const ImagePreviewContext = createContext<ImagePreviewContextValue | null>(null)

function clampZoom(value: number) {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, Number(value.toFixed(2))))
}

function extensionFromMimeType(mimeType: string | undefined) {
  if (!mimeType) return 'png'
  if (mimeType.includes('jpeg')) return 'jpg'
  if (mimeType.includes('webp')) return 'webp'
  if (mimeType.includes('gif')) return 'gif'
  if (mimeType.includes('svg')) return 'svg'
  if (mimeType.includes('png')) return 'png'
  return 'png'
}

function fileNameFromSource(src: string) {
  try {
    if (src.startsWith('data:')) return ''
    const url = new URL(src, window.location.href)
    const lastPathSegment = url.pathname.split('/').filter(Boolean).pop()
    return lastPathSegment ? decodeURIComponent(lastPathSegment) : ''
  } catch {
    return ''
  }
}

function sanitizeDownloadFileName(fileName: string) {
  return fileName.trim().replace(/[\\/:*?"<>|]+/g, '-').replace(/\s+/g, ' ')
}

function getDownloadFileName(preview: ImagePreviewInput, mimeType?: string) {
  const candidate = sanitizeDownloadFileName(
    preview.fileName || preview.alt || fileNameFromSource(preview.src) || 'image'
  )
  if (/\.[a-z0-9]{2,5}$/i.test(candidate)) return candidate
  return `${candidate}.${extensionFromMimeType(mimeType)}`
}

async function fetchImageBlob(src: string) {
  const response = await fetch(src)
  if (!response.ok) {
    throw new Error(`Image request failed with status ${response.status}`)
  }
  return response.blob()
}

function imageBlobToPngBlob(blob: Blob) {
  return new Promise<Blob>((resolve, reject) => {
    const image = new Image()
    const objectUrl = URL.createObjectURL(blob)

    image.onload = () => {
      const canvas = document.createElement('canvas')
      canvas.width = image.naturalWidth || image.width
      canvas.height = image.naturalHeight || image.height
      const context = canvas.getContext('2d')

      if (!context) {
        URL.revokeObjectURL(objectUrl)
        reject(new Error('Canvas is not available'))
        return
      }

      context.drawImage(image, 0, 0)
      canvas.toBlob((pngBlob) => {
        URL.revokeObjectURL(objectUrl)
        if (pngBlob) {
          resolve(pngBlob)
        } else {
          reject(new Error('Unable to convert image'))
        }
      }, 'image/png')
    }

    image.onerror = () => {
      URL.revokeObjectURL(objectUrl)
      reject(new Error('Unable to load image'))
    }

    image.src = objectUrl
  })
}

async function getClipboardImageBlob(src: string) {
  const blob = await fetchImageBlob(src)
  if (blob.type === 'image/png') return blob
  return imageBlobToPngBlob(blob)
}

function blobToDataUrl(blob: Blob) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => {
      if (typeof reader.result === 'string') {
        resolve(reader.result)
      } else {
        reject(new Error('Unable to read image data'))
      }
    }
    reader.onerror = () => reject(reader.error ?? new Error('Unable to read image data'))
    reader.readAsDataURL(blob)
  })
}

async function copyImageWithHostClipboard(src: string) {
  const blob = await getClipboardImageBlob(src)
  const dataUrl = await blobToDataUrl(blob)
  await hostClient.clipboard.writeImage({ dataUrl })
}

async function copyImageWithBrowserClipboard(src: string) {
  if (!navigator.clipboard?.write || typeof ClipboardItem === 'undefined') {
    throw new Error('Image clipboard is not available')
  }

  const blob = await getClipboardImageBlob(src)
  await navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })])
}

export function ImagePreviewProvider({ children }: { children: ReactNode }) {
  const { t } = useFrontendConfig()
  const dialogRef = useRef<HTMLDivElement>(null)
  const [preview, setPreview] = useState<ImagePreviewInput | null>(null)
  const [zoom, setZoom] = useState(1)
  const [copyState, setCopyState] = useState<CopyState>('idle')
  const [noticeMessage, setNoticeMessage] = useState('')
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null)

  const openImagePreview = useCallback((input: ImagePreviewInput) => {
    if (!input.src) return
    setPreview(input)
    setZoom(1)
    setCopyState('idle')
    setNoticeMessage('')
    setContextMenu(null)
  }, [])

  const closeImagePreview = useCallback(() => {
    setPreview(null)
    setCopyState('idle')
    setNoticeMessage('')
    setContextMenu(null)
  }, [])

  const showImagePreviewNotice = useCallback((message: string) => {
    setCopyState('idle')
    setNoticeMessage(message)
  }, [])

  const zoomOut = useCallback(() => {
    setZoom((currentZoom) => clampZoom(currentZoom - ZOOM_STEP))
  }, [])

  const zoomIn = useCallback(() => {
    setZoom((currentZoom) => clampZoom(currentZoom + ZOOM_STEP))
  }, [])

  const resetZoom = useCallback(() => {
    setZoom(1)
  }, [])

  const downloadImage = useCallback(async () => {
    if (!preview) return

    let objectUrl: string | undefined
    try {
      const blob = await fetchImageBlob(preview.src)
      objectUrl = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = objectUrl
      link.download = getDownloadFileName(preview, blob.type)
      document.body.append(link)
      link.click()
      link.remove()
    } catch {
      const link = document.createElement('a')
      link.href = preview.src
      link.download = getDownloadFileName(preview)
      document.body.append(link)
      link.click()
      link.remove()
    } finally {
      if (typeof objectUrl === 'string') {
        const urlToRevoke = objectUrl
        window.setTimeout(() => URL.revokeObjectURL(urlToRevoke), 1000)
      }
    }
  }, [preview])

  const copyImageToClipboard = useCallback(async (src: string) => {
    if (!src || copyState === 'copying') return false
    setCopyState('copying')
    setNoticeMessage('')
    setContextMenu(null)

    try {
      await copyImageWithHostClipboard(src)
      setCopyState('copied')
      return true
    } catch (error) {
      try {
        await copyImageWithBrowserClipboard(src)
        setCopyState('copied')
        return true
      } catch (fallbackError) {
        console.error('Failed to copy image', error, fallbackError)
        setCopyState('failed')
        return false
      }
    }
  }, [copyState])

  const copyImage = useCallback(async () => {
    if (!preview) return
    await copyImageToClipboard(preview.src)
  }, [copyImageToClipboard, preview])

  const value = useMemo(
    () => ({ copyImageToClipboard, openImagePreview, showImagePreviewNotice }),
    [copyImageToClipboard, openImagePreview, showImagePreviewNotice]
  )

  useEffect(() => {
    if (!preview) return
    dialogRef.current?.focus()
  }, [preview])

  useEffect(() => {
    if (!preview) return

    const handleKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'c') {
        event.preventDefault()
        void copyImageToClipboard(preview.src)
        return
      }
      if (event.key === 'Escape') {
        event.preventDefault()
        closeImagePreview()
        return
      }
      if (event.key === '+' || event.key === '=') {
        event.preventDefault()
        zoomIn()
        return
      }
      if (event.key === '-' || event.key === '_') {
        event.preventDefault()
        zoomOut()
        return
      }
      if (event.key === '0') {
        event.preventDefault()
        resetZoom()
      }
    }

    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [closeImagePreview, copyImageToClipboard, preview, resetZoom, zoomIn, zoomOut])

  useEffect(() => {
    if (copyState !== 'copied' && copyState !== 'failed') return
    const timeoutId = window.setTimeout(() => setCopyState('idle'), 1600)
    return () => window.clearTimeout(timeoutId)
  }, [copyState])

  useEffect(() => {
    if (!noticeMessage) return
    const timeoutId = window.setTimeout(() => setNoticeMessage(''), 1600)
    return () => window.clearTimeout(timeoutId)
  }, [noticeMessage])

  const statusMessage =
    noticeMessage ||
    (copyState === 'copying'
      ? t('imagePreview.copying')
      : copyState === 'copied'
        ? t('imagePreview.copied')
        : copyState === 'failed'
          ? t('imagePreview.copyFailed')
          : '')

  const handleBackdropMouseDown = (event: ReactMouseEvent<HTMLDivElement>) => {
    setContextMenu(null)
    if (event.currentTarget === event.target) {
      closeImagePreview()
    }
  }

  const handleImageContextMenu = (event: ReactMouseEvent<HTMLImageElement>) => {
    event.preventDefault()
    const menuWidth = 170
    const menuHeight = 56
    setContextMenu({
      x: Math.max(8, Math.min(event.clientX, window.innerWidth - menuWidth - 8)),
      y: Math.max(8, Math.min(event.clientY, window.innerHeight - menuHeight - 8))
    })
  }

  const handlePreviewImageError = () => {
    setPreview(null)
    setContextMenu(null)
    showImagePreviewNotice(t('imagePreview.originalMissing'))
  }

  const handleWheel = (event: ReactWheelEvent<HTMLDivElement>) => {
    event.preventDefault()
    setContextMenu(null)
    setZoom((currentZoom) =>
      clampZoom(currentZoom + (event.deltaY < 0 ? ZOOM_STEP : -ZOOM_STEP))
    )
  }

  return (
    <ImagePreviewContext.Provider value={value}>
      {children}
      {preview && (
        <div
          aria-label={preview.alt || t('imagePreview.title')}
          aria-modal="true"
          className="image-preview"
          onMouseDown={handleBackdropMouseDown}
          ref={dialogRef}
          role="dialog"
          tabIndex={-1}
        >
          <div className="image-preview__toolbar">
            <button
              aria-label={t('imagePreview.download')}
              className="image-preview__icon-button"
              onClick={() => void downloadImage()}
              title={t('imagePreview.download')}
              type="button"
            >
              <Download aria-hidden="true" />
            </button>
            <button
              aria-label={t('imagePreview.close')}
              className="image-preview__icon-button"
              onClick={closeImagePreview}
              title={t('imagePreview.close')}
              type="button"
            >
              <X aria-hidden="true" />
            </button>
          </div>

          <div
            className="image-preview__stage"
            onMouseDown={handleBackdropMouseDown}
            onWheel={handleWheel}
          >
            <img
              alt={preview.alt || ''}
              className="image-preview__image"
              draggable={false}
              onContextMenu={handleImageContextMenu}
              onError={handlePreviewImageError}
              src={preview.src}
              style={{ transform: `scale(${zoom})` }}
            />
          </div>

          <div className="image-preview__zoom-controls">
            <button
              aria-label={t('imagePreview.zoomOut')}
              className="image-preview__zoom-button"
              disabled={zoom <= MIN_ZOOM}
              onClick={zoomOut}
              title={t('imagePreview.zoomOut')}
              type="button"
            >
              <Minus aria-hidden="true" />
            </button>
            <button className="image-preview__zoom-value" onClick={resetZoom} type="button">
              {Math.round(zoom * 100)}%
            </button>
            <button
              aria-label={t('imagePreview.zoomIn')}
              className="image-preview__zoom-button"
              disabled={zoom >= MAX_ZOOM}
              onClick={zoomIn}
              title={t('imagePreview.zoomIn')}
              type="button"
            >
              <Plus aria-hidden="true" />
            </button>
          </div>

          {contextMenu && (
            <div
              className="image-preview__context-menu"
              onMouseDown={(event) => event.stopPropagation()}
              style={{ left: contextMenu.x, top: contextMenu.y }}
            >
              <button onClick={copyImage} type="button">
                <Copy aria-hidden="true" />
                <span>{t('imagePreview.copy')}</span>
              </button>
            </div>
          )}

        </div>
      )}
      {statusMessage && (
        <div className="image-preview__status" role="status">
          {statusMessage}
        </div>
      )}
    </ImagePreviewContext.Provider>
  )
}

export function useImagePreview() {
  const context = useContext(ImagePreviewContext)
  return context?.openImagePreview ?? (() => {})
}

export function useImageClipboard() {
  const context = useContext(ImagePreviewContext)
  return context?.copyImageToClipboard ?? (async () => false)
}

export function useImagePreviewNotice() {
  const context = useContext(ImagePreviewContext)
  return context?.showImagePreviewNotice ?? (() => {})
}
