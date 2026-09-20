import { RotateCcw, X } from 'lucide-react'
import { WorkspaceFileTypeIcon } from '../../../components/files/WorkspaceFileTypeIcon'
import type { ComposerAttachment } from '../chatAttachments'
import './AttachmentCards.css'

export type ComposerAttachmentPresentation = Pick<
  ComposerAttachment,
  'id' | 'kind' | 'name' | 'sizeBytes' | 'previewUrl'
> & {
  importState?: 'importing' | 'failed'
  importedBytes?: number
  error?: string
}

interface ComposerAttachmentsProps {
  attachments: readonly ComposerAttachmentPresentation[]
  label: string
  removeLabel: string
  cancelLabel?: string
  retryLabel?: string
  failedLabel?: string
  onRetry?: (id: string) => void
  onPreviewAttachment?: (id: string) => void
  onRemove?: (id: string) => void
  onPreview?: (image: { alt: string; fileName: string; src: string }) => void
  /** Message and queue cards reuse the composer layout but do not expose destructive actions. */
  variant?: 'composer' | 'message' | 'queue'
  messageId?: string
}

function attachmentColumns(
  attachments: readonly ComposerAttachmentPresentation[]
): ComposerAttachmentPresentation[][] {
  const columns: ComposerAttachmentPresentation[][] = []
  for (const attachment of attachments) {
    const last = columns.at(-1)
    if (attachment.kind === 'file' && last?.[0].kind === 'file' && last.length < 4) {
      last.push(attachment)
    } else {
      columns.push([attachment])
    }
  }
  return columns
}

function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  const units = ['KiB', 'MiB', 'GiB', 'TiB']
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)) - 1, units.length - 1)
  return `${Number((bytes / 1024 ** (index + 1)).toFixed(1))} ${units[index]}`
}

function AttachmentName({ name }: { name: string }) {
  const separator = name.lastIndexOf('.')
  const hasShortExtension = separator > 0 && name.length - separator <= 16
  return (
    <span className="composer-attachment__name">
      <span className="composer-attachment__basename">
        {hasShortExtension ? name.slice(0, separator) : name}
      </span>
      {hasShortExtension && (
        <span className="composer-attachment__extension">{name.slice(separator)}</span>
      )}
    </span>
  )
}

export function ComposerAttachments({
  attachments,
  label,
  removeLabel,
  cancelLabel = removeLabel,
  retryLabel = 'Retry',
  failedLabel = '',
  onRetry,
  onPreviewAttachment,
  onRemove,
  onPreview,
  variant = 'composer',
  messageId
}: ComposerAttachmentsProps) {
  const listClassName = [
    variant === 'composer' ? 'chat-composer__attachments' : 'attachment-card-list',
    variant !== 'composer' ? `attachment-card-list--${variant}` : ''
  ]
    .filter(Boolean)
    .join(' ')
  return (
    <div className={listClassName} aria-label={label}>
      {attachmentColumns(attachments).map((column) => (
        <div
          className="composer-attachment-column attachment-card-column"
          data-kind={column[0].kind}
          key={column[0].id}
        >
          {column.map((attachment) => (
            <div
              className={`composer-attachment attachment-card${variant === 'message' ? ' chat-message-attachment' : ''}`}
              data-chat-attachment-id={variant === 'message' ? attachment.id : undefined}
              data-chat-attachment-message-id={variant === 'message' ? messageId : undefined}
              data-kind={attachment.kind}
              data-import-state={attachment.importState}
              key={attachment.id}
              onClick={(event) => {
                if (
                  variant === 'message' &&
                  attachment.kind === 'image' &&
                  event.target === event.currentTarget
                ) {
                  onPreviewAttachment?.(attachment.id)
                }
              }}
              title={`${attachment.name} · ${fileSize(attachment.sizeBytes)}${attachment.error ? ` · ${attachment.error}` : ''}`}
            >
              {attachment.kind === 'image' && attachment.previewUrl ? (
                <button
                  className="composer-attachment__image-button"
                  onClick={() => {
                    if (onPreviewAttachment) {
                      onPreviewAttachment(attachment.id)
                      return
                    }
                    onPreview?.({
                      alt: attachment.name,
                      fileName: attachment.name,
                      src: attachment.previewUrl ?? ''
                    })
                  }}
                  title={attachment.name}
                  type="button"
                >
                  <img
                    className="composer-attachment__thumbnail"
                    src={attachment.previewUrl}
                    alt={attachment.name}
                  />
                </button>
              ) : (
                <>
                  <WorkspaceFileTypeIcon
                    className="composer-attachment__icon"
                    path={attachment.name}
                  />
                  <div className="composer-attachment__content">
                    <AttachmentName name={attachment.name} />
                    {attachment.importState === 'importing' && (
                      <span
                        className="composer-attachment__progress"
                        role="progressbar"
                        aria-label={attachment.name}
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-valuenow={
                          attachment.sizeBytes > 0
                            ? Math.min(
                                100,
                                Math.floor(
                                  ((attachment.importedBytes ?? 0) / attachment.sizeBytes) * 100
                                )
                              )
                            : 0
                        }
                      >
                        {attachment.sizeBytes > 0
                          ? Math.min(
                              100,
                              Math.floor(
                                ((attachment.importedBytes ?? 0) / attachment.sizeBytes) * 100
                              )
                            )
                          : 0}
                        %
                      </span>
                    )}
                    {attachment.importState === 'failed' && onRetry && (
                      <button
                        className="composer-attachment__retry"
                        type="button"
                        aria-label={`${retryLabel} ${attachment.name}`}
                        title={attachment.error ?? failedLabel}
                        onClick={() => onRetry(attachment.id)}
                      >
                        <RotateCcw aria-hidden="true" />
                      </button>
                    )}
                  </div>
                </>
              )}
              {onRemove && (
                <button
                  type="button"
                  className="composer-attachment__remove"
                  aria-label={`${attachment.importState === 'importing' ? cancelLabel : removeLabel} ${attachment.name}`}
                  onClick={() => onRemove(attachment.id)}
                >
                  <X aria-hidden="true" />
                </button>
              )}
            </div>
          ))}
        </div>
      ))}
    </div>
  )
}
