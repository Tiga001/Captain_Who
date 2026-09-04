import { useEffect, useMemo, useRef, useState } from 'react'
import type { CSSProperties, ReactNode, RefObject } from 'react'
import type { WorkspaceFilePreviewResult, WorkspaceTextFileContent } from '@mycopilot/protocol'
import { AlertCircle, FileQuestion, FileWarning, FolderOpen, LoaderCircle } from 'lucide-react'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { formatTranslation } from '../../config/translationFormat'
import { resolveGitReviewFileLanguageDescriptor } from '../gitReview/syntax/fileLanguageRegistry'
import { useGitReviewSyntaxHighlight } from '../gitReview/syntaxHighlighting/useGitReviewSyntaxHighlight'
import type { GitReviewSyntaxHighlightState } from '../gitReview/syntaxHighlighting/useGitReviewSyntaxHighlight'
import { readWorkspaceFilePreview } from './filesClient'
import { WorkspaceMarkdownPreview } from './WorkspaceMarkdownPreview'
import { WorkspacePdfPreview } from './WorkspacePdfPreview'
import {
  getWorkspaceOfficeDocumentType,
  isWorkspaceMarkdownFile
} from './workspaceFilePreviewTypes'
import type { WorkspaceMarkdownView } from './workspaceFilePreviewTypes'

const MAX_RENDERED_LINES = 5_000

type PreviewState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'error' }
  | {
      preview: WorkspaceFilePreviewResult
      status: 'ready'
    }

interface WorkspaceFilePreviewProps {
  isActive: boolean
  markdownAnchor?: string
  markdownView: WorkspaceMarkdownView
  onOpenFile: (path: string, anchor?: string) => void
  onPdfPageChange: (page: number) => void
  path: string | null
  pdfPage: number
  projectId: string
  wrapLines: boolean
}

export function WorkspaceFilePreview({
  isActive,
  markdownAnchor,
  markdownView,
  onOpenFile,
  onPdfPageChange,
  path,
  pdfPage,
  projectId,
  wrapLines
}: WorkspaceFilePreviewProps): ReactNode {
  const { t } = useFrontendConfig()
  const [retryToken, setRetryToken] = useState(0)
  const [state, setState] = useState<PreviewState>({ status: 'idle' })
  const requestSequenceRef = useRef(0)
  const scrollRef = useRef<HTMLDivElement>(null)
  const officeDocumentType = getWorkspaceOfficeDocumentType(path)
  const isMarkdown = isWorkspaceMarkdownFile(path)

  useEffect(() => {
    if (!path || officeDocumentType) {
      requestSequenceRef.current += 1
      setState({ status: 'idle' })
      return
    }
    if (!isActive) return

    const requestId = requestSequenceRef.current + 1
    requestSequenceRef.current = requestId
    setState({ status: 'loading' })
    scrollRef.current?.scrollTo({ left: 0, top: 0 })

    void (async () => {
      try {
        const request = { path, projectId }
        const preview = await readWorkspaceFilePreview(request)
        if (requestSequenceRef.current !== requestId) return
        setState({ preview, status: 'ready' })
      } catch {
        if (requestSequenceRef.current === requestId) setState({ status: 'error' })
      }
    })()

    return () => {
      if (requestSequenceRef.current === requestId) requestSequenceRef.current += 1
    }
  }, [isActive, officeDocumentType, path, projectId, retryToken])

  if (!path) {
    return (
      <FilePreviewMessage
        description={t('files.empty.description')}
        icon={<FolderOpen aria-hidden="true" />}
        title={t('files.empty.title')}
      />
    )
  }

  if (officeDocumentType) {
    return (
      <FilePreviewMessage
        description={t('files.preview.officeDescription')}
        title={formatTranslation(t, 'files.preview.officeTitle', { type: officeDocumentType })}
        variant="unsupported"
      />
    )
  }

  if (state.status === 'idle') {
    return (
      <FilePreviewMessage
        description={t('files.preview.loading')}
        icon={<LoaderCircle className="files-panel__spinner" aria-hidden="true" />}
      />
    )
  }

  if (state.status === 'loading') {
    return (
      <FilePreviewMessage
        description={t('files.preview.loading')}
        icon={<LoaderCircle className="files-panel__spinner" aria-hidden="true" />}
      />
    )
  }

  if (state.status === 'error') {
    return (
      <FilePreviewMessage
        action={
          <button type="button" onClick={() => setRetryToken((token) => token + 1)}>
            {t('files.retry')}
          </button>
        }
        description={t('files.preview.error')}
        icon={<AlertCircle aria-hidden="true" />}
        title={t('files.preview.errorTitle')}
      />
    )
  }

  if (state.preview.text) {
    if (isMarkdown && markdownView === 'preview') {
      return (
        <WorkspaceMarkdownPreview
          anchor={markdownAnchor}
          content={state.preview.text}
          onOpenFile={onOpenFile}
          path={path}
          projectId={projectId}
          scrollRef={scrollRef}
        />
      )
    }
    return (
      <TextFilePreview
        content={state.preview.text}
        path={path}
        projectId={projectId}
        scrollRef={scrollRef}
        wrapLines={wrapLines}
      />
    )
  }

  if (state.preview.image) {
    return (
      <div className="files-panel__preview-scroll files-panel__image-preview" ref={scrollRef}>
        <img
          alt={path.split('/').at(-1) ?? path}
          src={`data:${state.preview.image.mimeType};base64,${state.preview.image.data}`}
        />
      </div>
    )
  }

  if (state.preview.pdf) {
    return (
      <WorkspacePdfPreview
        content={state.preview.pdf}
        initialPage={pdfPage}
        key={`${state.preview.pdf.path}:${state.preview.pdf.modifiedAtMs}`}
        onPageChange={onPdfPageChange}
        onRetry={() => setRetryToken((token) => token + 1)}
        path={path}
      />
    )
  }

  const message =
    state.preview.metadata.previewKind === 'too-large'
      ? t('files.preview.tooLarge')
      : state.preview.metadata.previewKind === 'binary'
        ? t('files.preview.binary')
        : t('files.preview.unsupported')
  const icon =
    state.preview.metadata.previewKind === 'too-large' ? (
      <FileWarning aria-hidden="true" />
    ) : (
      <FileQuestion aria-hidden="true" />
    )
  return <FilePreviewMessage description={message} icon={icon} title={path.split('/').at(-1)} />
}

interface TextFilePreviewProps {
  content: WorkspaceTextFileContent
  path: string
  projectId: string
  scrollRef: RefObject<HTMLDivElement | null>
  wrapLines: boolean
}

function TextFilePreview({
  content,
  path,
  projectId,
  scrollRef,
  wrapLines
}: TextFilePreviewProps): ReactNode {
  const { t } = useFrontendConfig()
  const lines = useMemo(() => content.content.split('\n'), [content.content])
  const language = useMemo(
    () => resolveGitReviewFileLanguageDescriptor({ newPath: path, oldPath: path }).newLanguageId,
    [path]
  )
  const highlightState = useGitReviewSyntaxHighlight({
    cacheKey: `workspace-file:${projectId}:${path}:${content.modifiedAtMs}:${content.sizeBytes}`,
    code: content.content,
    enabled: lines.length <= MAX_RENDERED_LINES && language !== 'text',
    language
  })

  if (lines.length > MAX_RENDERED_LINES) {
    return (
      <FilePreviewMessage
        description={t('files.preview.tooManyLines')}
        icon={<FileWarning aria-hidden="true" />}
        title={path.split('/').at(-1)}
      />
    )
  }

  return (
    <div
      className="files-panel__preview-scroll files-panel__code"
      data-wrap-lines={wrapLines ? 'true' : undefined}
      ref={scrollRef}
    >
      <div className="files-panel__code-lines" role="table" aria-label={path}>
        {lines.map((line, index) => (
          <div className="files-panel__code-line" role="row" key={index}>
            <span
              className="files-panel__line-number"
              role="rowheader"
              aria-label={`Line ${index + 1}`}
            >
              {index + 1}
            </span>
            <code role="cell">{renderHighlightedLine(highlightState, index, line)}</code>
          </div>
        ))}
      </div>
    </div>
  )
}

function renderHighlightedLine(
  state: GitReviewSyntaxHighlightState,
  lineIndex: number,
  content: string
): ReactNode {
  if (state.status !== 'ready' || state.result.mode !== 'highlighted') return content || ' '
  const line = state.result.lines[lineIndex]
  if (!line || line.line !== lineIndex) return content || ' '
  if (line.tokens.map((token) => token.content).join('') !== content) return content || ' '
  return line.tokens.length > 0
    ? line.tokens.map((token) => (
        <span
          data-syntax-token="true"
          key={token.start}
          style={token.color ? ({ color: token.color } satisfies CSSProperties) : undefined}
        >
          {token.content}
        </span>
      ))
    : ' '
}

interface FilePreviewMessageProps {
  action?: ReactNode
  description: string
  icon?: ReactNode
  title?: string
  variant?: 'default' | 'unsupported'
}

function FilePreviewMessage({
  action,
  description,
  icon,
  title,
  variant = 'default'
}: FilePreviewMessageProps): ReactNode {
  return (
    <div className="files-panel__center-state" data-variant={variant}>
      {icon && <div className="files-panel__center-icon">{icon}</div>}
      {title && <h2>{title}</h2>}
      <p>{description}</p>
      {action}
    </div>
  )
}
