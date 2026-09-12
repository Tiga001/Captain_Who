import { Children, Fragment, isValidElement, useEffect, useId, useMemo, useState } from 'react'
import type { ComponentProps, ReactNode, RefObject } from 'react'
import ReactMarkdown from 'react-markdown'
import type { Components } from 'react-markdown'
import rehypeKatex from 'rehype-katex'
import rehypeSlug from 'rehype-slug'
import remarkBreaks from 'remark-breaks'
import remarkGfm from 'remark-gfm'
import remarkMath from 'remark-math'
import type { WorkspaceTextFileContent } from '@mycopilot/protocol'
import { ChevronDown, ChevronUp } from 'lucide-react'
import 'katex/dist/katex.min.css'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import {
  GIT_REVIEW_SYNTAX_LANGUAGE_IDS,
  type GitReviewSyntaxLanguageId
} from '../gitReview/syntax/fileLanguageRegistry'
import { useGitReviewSyntaxHighlight } from '../gitReview/syntaxHighlighting/useGitReviewSyntaxHighlight'
import { openWorkspaceExternalLink, readWorkspaceFilePreview } from './filesClient'
import {
  parseWorkspaceMarkdownDocument,
  type WorkspaceMarkdownDocument,
  type WorkspaceMarkdownMetadataEntry
} from './workspaceMarkdownDocument'
import { resolveWorkspaceMarkdownLink } from './workspaceMarkdownLinks'

const COLLAPSED_METADATA_ENTRY_COUNT = 8
const REMARK_PLUGINS = [remarkGfm, remarkMath, remarkBreaks]
const REHYPE_PLUGINS = [rehypeSlug, rehypeKatex]
const SUPPORTED_LANGUAGE_IDS = new Set<string>(GIT_REVIEW_SYNTAX_LANGUAGE_IDS)

const LANGUAGE_ALIASES: Readonly<Record<string, GitReviewSyntaxLanguageId>> = {
  bash: 'shellscript',
  cjs: 'javascript',
  cs: 'csharp',
  js: 'javascript',
  md: 'markdown',
  mjs: 'javascript',
  plaintext: 'text',
  py: 'python',
  rb: 'ruby',
  sh: 'shellscript',
  shell: 'shellscript',
  ts: 'typescript',
  txt: 'text',
  yml: 'yaml',
  zsh: 'shellscript'
}

interface WorkspaceMarkdownPreviewProps {
  assistantMessageId?: string
  folderId?: string
  anchor?: string
  content: WorkspaceTextFileContent
  onOpenFile: (path: string, anchor?: string) => void
  path: string
  projectId: string
  scrollRef: RefObject<HTMLDivElement | null>
}

export function WorkspaceMarkdownPreview({
  assistantMessageId,
  folderId,
  anchor,
  content,
  onOpenFile,
  path,
  projectId,
  scrollRef
}: WorkspaceMarkdownPreviewProps): ReactNode {
  const document = useMemo(() => parseWorkspaceMarkdownDocument(content.content), [content.content])
  const components = useMemo<Components>(
    () => ({
      a: (props) => (
        <WorkspaceMarkdownAnchor
          {...props}
          currentPath={path}
          onOpenFile={onOpenFile}
          onScrollToAnchor={(nextAnchor) => scrollToMarkdownAnchor(scrollRef, nextAnchor)}
        />
      ),
      img: (props) => (
        <WorkspaceMarkdownImage
          {...props}
          currentPath={path}
          projectId={projectId}
          folderId={folderId}
          assistantMessageId={assistantMessageId}
        />
      ),
      input: ({ type, ...props }) => (
        <input {...props} disabled={type === 'checkbox'} type={type} />
      ),
      pre: WorkspaceMarkdownPre
    }),
    [assistantMessageId, folderId, onOpenFile, path, projectId, scrollRef]
  )

  useEffect(() => {
    if (!anchor) return
    const frameId = window.requestAnimationFrame(() => scrollToMarkdownAnchor(scrollRef, anchor))
    return () => window.cancelAnimationFrame(frameId)
  }, [anchor, document.body, scrollRef])

  return (
    <div className="files-panel__preview-scroll files-panel__markdown-scroll" ref={scrollRef}>
      <article className="files-panel__markdown" aria-label={path}>
        <WorkspaceMarkdownMetadata document={document} key={path} />
        <ReactMarkdown
          components={components}
          rehypePlugins={REHYPE_PLUGINS}
          remarkPlugins={REMARK_PLUGINS}
        >
          {document.body}
        </ReactMarkdown>
      </article>
    </div>
  )
}

function WorkspaceMarkdownMetadata({ document }: { document: WorkspaceMarkdownDocument }) {
  const { t } = useFrontendConfig()
  const [expanded, setExpanded] = useState(false)
  const titleId = useId()
  const hasAdditionalEntries = document.metadata.length > COLLAPSED_METADATA_ENTRY_COUNT
  const visibleEntries = expanded
    ? document.metadata
    : document.metadata.slice(0, COLLAPSED_METADATA_ENTRY_COUNT)

  if (document.metadata.length === 0 && !document.metadataError) return null

  return (
    <section
      className="files-panel__markdown-metadata"
      aria-labelledby={titleId}
      data-error={document.metadataError ? 'true' : undefined}
    >
      <h2 id={titleId}>{t('files.markdown.metadata')}</h2>
      {document.metadataError && (
        <p className="files-panel__markdown-metadata-error">
          {document.metadataError === 'too-large'
            ? t('files.markdown.metadataTooLarge')
            : t('files.markdown.metadataInvalid')}
        </p>
      )}
      {visibleEntries.length > 0 && (
        <dl>
          {visibleEntries.map((entry, index) => (
            <MetadataEntry entry={entry} key={`${entry.key}:${index}`} />
          ))}
        </dl>
      )}
      {document.metadataTruncated && (
        <p className="files-panel__markdown-metadata-note">
          {t('files.markdown.metadataTruncated')}
        </p>
      )}
      {hasAdditionalEntries && (
        <button
          className="files-panel__markdown-metadata-toggle"
          type="button"
          aria-expanded={expanded}
          onClick={() => setExpanded((current) => !current)}
        >
          {expanded ? <ChevronUp aria-hidden="true" /> : <ChevronDown aria-hidden="true" />}
          <span>{expanded ? t('files.markdown.showLess') : t('files.markdown.showMore')}</span>
        </button>
      )}
    </section>
  )
}

function MetadataEntry({ entry }: { entry: WorkspaceMarkdownMetadataEntry }) {
  return (
    <div className="files-panel__markdown-metadata-row">
      <dt>{entry.key}</dt>
      <dd>
        {entry.value.kind === 'list' ? (
          entry.value.items.length > 0 ? (
            <span className="files-panel__markdown-metadata-list">
              {entry.value.items.map((item, index) => (
                <span key={`${item}:${index}`}>{item}</span>
              ))}
            </span>
          ) : (
            '[]'
          )
        ) : (
          entry.value.text
        )}
      </dd>
    </div>
  )
}

interface WorkspaceMarkdownAnchorProps extends ComponentProps<'a'> {
  currentPath: string
  onOpenFile: (path: string, anchor?: string) => void
  onScrollToAnchor: (anchor: string) => void
}

function WorkspaceMarkdownAnchor({
  children,
  currentPath,
  href,
  onOpenFile,
  onScrollToAnchor,
  ...props
}: WorkspaceMarkdownAnchorProps) {
  const { t } = useFrontendConfig()
  const target = resolveWorkspaceMarkdownLink(currentPath, href)
  if (target.kind === 'unsupported') {
    return (
      <span
        className="files-panel__markdown-link-unavailable"
        title={t('files.markdown.linkUnavailable')}
      >
        {children}
      </span>
    )
  }

  const renderedHref =
    target.kind === 'external'
      ? target.url
      : target.kind === 'anchor'
        ? `#${encodeURIComponent(target.anchor)}`
        : href

  return (
    <a
      {...props}
      href={renderedHref}
      onClick={(event) => {
        event.preventDefault()
        if (target.kind === 'external') {
          void openWorkspaceExternalLink(target.url).catch(() => undefined)
        } else if (target.kind === 'anchor') {
          onScrollToAnchor(target.anchor)
        } else if (target.path === currentPath && target.anchor) {
          onScrollToAnchor(target.anchor)
        } else {
          onOpenFile(target.path, target.anchor)
        }
      }}
      rel={target.kind === 'external' ? 'noreferrer' : undefined}
      target={target.kind === 'external' ? '_blank' : undefined}
    >
      {children}
    </a>
  )
}

interface WorkspaceMarkdownImageProps extends Omit<ComponentProps<'img'>, 'src'> {
  assistantMessageId?: string
  folderId?: string
  currentPath: string
  projectId: string
  src?: string
}

function WorkspaceMarkdownImage({
  assistantMessageId,
  folderId,
  alt,
  currentPath,
  projectId,
  src,
  ...props
}: WorkspaceMarkdownImageProps) {
  const { t } = useFrontendConfig()
  const target = useMemo(() => resolveWorkspaceMarkdownLink(currentPath, src), [currentPath, src])
  const [localImageUrl, setLocalImageUrl] = useState<string | null>(null)
  const [localImageFailed, setLocalImageFailed] = useState(false)

  useEffect(() => {
    if (target.kind !== 'workspace-file') {
      setLocalImageUrl(null)
      setLocalImageFailed(false)
      return
    }

    let cancelled = false
    setLocalImageUrl(null)
    setLocalImageFailed(false)
    void readWorkspaceFilePreview({
      path: target.path,
      projectId,
      ...(folderId === undefined ? {} : { folderId }),
      ...(assistantMessageId === undefined ? {} : { assistantMessageId })
    })
      .then((preview) => {
        if (cancelled) return
        if (!preview.image) {
          setLocalImageFailed(true)
          return
        }
        setLocalImageUrl(`data:${preview.image.mimeType};base64,${preview.image.data}`)
      })
      .catch(() => {
        if (!cancelled) setLocalImageFailed(true)
      })
    return () => {
      cancelled = true
    }
  }, [assistantMessageId, folderId, projectId, target])

  if (target.kind === 'external') {
    return <img {...props} alt={alt ?? ''} src={target.url} />
  }
  if (target.kind !== 'workspace-file' || localImageFailed) {
    return (
      <span className="files-panel__markdown-image-placeholder" role="img" aria-label={alt ?? ''}>
        {alt || t('files.markdown.imageUnavailable')}
      </span>
    )
  }
  if (!localImageUrl) {
    return (
      <span className="files-panel__markdown-image-placeholder" role="status">
        {t('files.markdown.imageLoading')}
      </span>
    )
  }
  return <img {...props} alt={alt ?? ''} src={localImageUrl} />
}

function WorkspaceMarkdownPre({ children }: ComponentProps<'pre'>) {
  const codeElement = Children.toArray(children).find(
    (child) => isValidElement(child) && child.type === 'code'
  )
  if (!isValidElement(codeElement)) return <pre>{children}</pre>

  const codeProps = codeElement.props as { children?: ReactNode; className?: string }
  const language = /(?:^|\s)language-([^\s]+)/.exec(codeProps.className ?? '')?.[1]
  return (
    <WorkspaceMarkdownCodeBlock
      code={readMarkdownCodeText(codeProps.children).replace(/\n$/, '')}
      language={language}
    />
  )
}

function WorkspaceMarkdownCodeBlock({ code, language }: { code: string; language?: string }) {
  const languageId = resolveCodeLanguage(language)
  const cacheKey = useMemo(() => createCodeCacheKey(languageId, code), [code, languageId])
  const highlightState = useGitReviewSyntaxHighlight({
    cacheKey,
    code,
    enabled: languageId !== 'text' && code.length > 0,
    language: languageId
  })
  const highlightedLines =
    highlightState.status === 'ready' &&
    highlightState.result.mode === 'highlighted' &&
    reconstructHighlightedCode(highlightState.result.lines) === code
      ? highlightState.result.lines
      : null

  return (
    <pre data-language={language || undefined}>
      <code>
        {highlightedLines
          ? highlightedLines.map((line, lineIndex) => (
              <Fragment key={`${line.line}:${lineIndex}`}>
                {line.tokens.map((token, tokenIndex) => (
                  <span
                    key={`${token.start}:${token.end}:${tokenIndex}`}
                    style={token.color ? { color: token.color } : undefined}
                  >
                    {token.content}
                  </span>
                ))}
                {lineIndex < highlightedLines.length - 1 ? '\n' : null}
              </Fragment>
            ))
          : code}
      </code>
    </pre>
  )
}

function scrollToMarkdownAnchor(scrollRef: RefObject<HTMLDivElement | null>, anchor: string): void {
  const target = Array.from(scrollRef.current?.querySelectorAll<HTMLElement>('[id]') ?? []).find(
    (element) => element.id === anchor
  )
  target?.scrollIntoView({ block: 'start' })
}

function readMarkdownCodeText(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(readMarkdownCodeText).join('')
  if (!isValidElement(node)) return ''
  return readMarkdownCodeText((node.props as { children?: ReactNode }).children)
}

function resolveCodeLanguage(language: string | undefined): GitReviewSyntaxLanguageId {
  const normalized = language?.trim().toLowerCase() ?? ''
  const alias = LANGUAGE_ALIASES[normalized]
  if (alias) return alias
  return SUPPORTED_LANGUAGE_IDS.has(normalized) ? (normalized as GitReviewSyntaxLanguageId) : 'text'
}

function createCodeCacheKey(language: GitReviewSyntaxLanguageId, code: string): string {
  let hash = 2_166_136_261
  for (let index = 0; index < code.length; index += 1) {
    hash ^= code.charCodeAt(index)
    hash = Math.imul(hash, 16_777_619)
  }
  return `workspace-markdown:${language}:${code.length}:${(hash >>> 0).toString(36)}`
}

function reconstructHighlightedCode(
  lines: readonly { tokens: readonly { content: string }[] }[]
): string {
  return lines.map((line) => line.tokens.map((token) => token.content).join('')).join('\n')
}
