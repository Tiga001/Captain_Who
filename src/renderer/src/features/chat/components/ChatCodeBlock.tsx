import { Fragment, memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Check, Copy, WrapText } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import {
  GIT_REVIEW_SYNTAX_LANGUAGE_IDS,
  type GitReviewSyntaxLanguageId
} from '../../gitReview/syntax/fileLanguageRegistry'
import { useSyntaxHighlight, type SyntaxHighlightLine } from '../../syntaxHighlighting'
import { copyTextToClipboard } from './chatMessageItemUtils'

interface ChatCodeBlockProps {
  code: string
  language?: string
}

interface ResolvedLanguage {
  id: GitReviewSyntaxLanguageId
  label: string
}

interface HighlightSnapshot {
  code: string
  language: GitReviewSyntaxLanguageId
  lines: readonly SyntaxHighlightLine[]
}

const SUPPORTED_LANGUAGE_IDS = new Set<string>(GIT_REVIEW_SYNTAX_LANGUAGE_IDS)

const LANGUAGE_ALIASES: Readonly<Record<string, GitReviewSyntaxLanguageId>> = {
  bash: 'shellscript',
  cjs: 'javascript',
  cs: 'csharp',
  html5: 'html',
  js: 'javascript',
  md: 'markdown',
  mjs: 'javascript',
  plaintext: 'text',
  py: 'python',
  rb: 'ruby',
  sh: 'shellscript',
  shell: 'shellscript',
  text: 'text',
  ts: 'typescript',
  txt: 'text',
  yml: 'yaml',
  zsh: 'shellscript'
}

const LANGUAGE_LABELS: Partial<Record<GitReviewSyntaxLanguageId, string>> = {
  bat: 'Batch',
  csharp: 'C#',
  cpp: 'C++',
  css: 'CSS',
  csv: 'CSV',
  docker: 'Dockerfile',
  dotenv: 'Dotenv',
  graphql: 'GraphQL',
  html: 'HTML',
  javascript: 'JavaScript',
  json: 'JSON',
  json5: 'JSON5',
  jsonc: 'JSONC',
  jsx: 'JSX',
  latex: 'LaTeX',
  markdown: 'Markdown',
  mdx: 'MDX',
  'objective-c': 'Objective-C',
  'objective-cpp': 'Objective-C++',
  php: 'PHP',
  powershell: 'PowerShell',
  python: 'Python',
  r: 'R',
  rst: 'reStructuredText',
  shellscript: 'Shell',
  sql: 'SQL',
  tsx: 'TSX',
  tsv: 'TSV',
  typescript: 'TypeScript',
  vue: 'Vue',
  wasm: 'WebAssembly',
  xml: 'XML',
  yaml: 'YAML'
}

const HIGHLIGHT_SETTLE_DELAY_MS = 120
const COPIED_INDICATOR_DURATION_MS = 1_600

function resolveCodeLanguage(
  rawLanguage: string | undefined,
  plainTextLabel: string
): ResolvedLanguage {
  const trimmedLanguage = rawLanguage?.trim() ?? ''
  const normalized = trimmedLanguage.toLowerCase()
  const alias = LANGUAGE_ALIASES[normalized]
  const supportedId = SUPPORTED_LANGUAGE_IDS.has(normalized) ? normalized : undefined
  const id = alias ?? supportedId ?? 'text'
  const languageId = id as GitReviewSyntaxLanguageId
  const isPlainTextLabel =
    !trimmedLanguage || normalized === 'text' || normalized === 'txt' || normalized === 'plaintext'

  return {
    id: languageId,
    label:
      languageId === 'text'
        ? isPlainTextLabel
          ? plainTextLabel
          : trimmedLanguage
        : (LANGUAGE_LABELS[languageId] ?? trimmedLanguage)
  }
}

function useSettledCode(code: string) {
  const [settledCode, setSettledCode] = useState(code)

  useEffect(() => {
    if (settledCode === code) return
    const timeoutId = window.setTimeout(() => setSettledCode(code), HIGHLIGHT_SETTLE_DELAY_MS)
    return () => window.clearTimeout(timeoutId)
  }, [code, settledCode])

  return settledCode
}

function createCodeCacheKey(language: GitReviewSyntaxLanguageId, code: string) {
  let hash = 2_166_136_261
  for (let index = 0; index < code.length; index += 1) {
    hash ^= code.charCodeAt(index)
    hash = Math.imul(hash, 16_777_619)
  }
  return `chat-code:${language}:${code.length}:${(hash >>> 0).toString(36)}`
}

function reconstructHighlightedCode(lines: readonly SyntaxHighlightLine[]) {
  return lines.map((line) => line.tokens.map((token) => token.content).join('')).join('\n')
}

function HighlightedCode({ lines }: { lines: readonly SyntaxHighlightLine[] }) {
  return (
    <>
      {lines.map((line, lineIndex) => (
        <Fragment key={`${line.line}:${lineIndex}`}>
          {line.tokens.map((token, tokenIndex) => (
            <span
              key={`${token.start}:${token.end}:${tokenIndex}`}
              style={token.color ? { color: token.color } : undefined}
            >
              {token.content}
            </span>
          ))}
          {lineIndex < lines.length - 1 ? '\n' : null}
        </Fragment>
      ))}
    </>
  )
}

function ChatCodeBlockComponent({ code, language }: ChatCodeBlockProps) {
  const { t } = useFrontendConfig()
  const [wrapped, setWrapped] = useState(false)
  const [copied, setCopied] = useState(false)
  const copiedTimeoutRef = useRef<number | null>(null)
  const resolvedLanguage = useMemo(
    () => resolveCodeLanguage(language, t('chat.codeBlock.plainText')),
    [language, t]
  )
  const settledCode = useSettledCode(code)
  const cacheKey = useMemo(
    () => createCodeCacheKey(resolvedLanguage.id, settledCode),
    [resolvedLanguage.id, settledCode]
  )
  const highlightState = useSyntaxHighlight({
    cacheKey,
    code: settledCode,
    enabled: resolvedLanguage.id !== 'text' && settledCode.length > 0,
    language: resolvedLanguage.id
  })
  const settledHighlightedLines = useMemo(() => {
    if (highlightState.status !== 'ready') return null
    if (highlightState.result.mode !== 'highlighted') return null
    if (reconstructHighlightedCode(highlightState.result.lines) !== settledCode) return null
    return highlightState.result.lines
  }, [highlightState, settledCode])
  const [highlightSnapshot, setHighlightSnapshot] = useState<HighlightSnapshot | null>(null)

  useLayoutEffect(() => {
    if (!settledHighlightedLines) return
    setHighlightSnapshot((current) => {
      if (
        current?.code === settledCode &&
        current.language === resolvedLanguage.id &&
        current.lines === settledHighlightedLines
      ) {
        return current
      }
      return {
        code: settledCode,
        language: resolvedLanguage.id,
        lines: settledHighlightedLines
      }
    })
  }, [resolvedLanguage.id, settledCode, settledHighlightedLines])

  const exactHighlightedLines = settledCode === code ? settledHighlightedLines : null
  const highlightedPrefix =
    !exactHighlightedLines &&
    highlightSnapshot?.language === resolvedLanguage.id &&
    code.startsWith(highlightSnapshot.code)
      ? highlightSnapshot
      : null

  useEffect(
    () => () => {
      if (copiedTimeoutRef.current !== null) {
        window.clearTimeout(copiedTimeoutRef.current)
      }
    },
    []
  )

  const handleCopy = async () => {
    try {
      await copyTextToClipboard(code)
      setCopied(true)
      if (copiedTimeoutRef.current !== null) {
        window.clearTimeout(copiedTimeoutRef.current)
      }
      copiedTimeoutRef.current = window.setTimeout(() => {
        setCopied(false)
        copiedTimeoutRef.current = null
      }, COPIED_INDICATOR_DURATION_MS)
    } catch (error) {
      console.error('Failed to copy Markdown code block', error)
    }
  }

  return (
    <section className={`chat-code-block${wrapped ? ' is-wrapped' : ''}`}>
      <div className="chat-code-block__header">
        <span className="chat-code-block__language">{resolvedLanguage.label}</span>
        <div className="chat-code-block__actions">
          <button
            aria-label={t('files.wrapLines')}
            aria-pressed={wrapped}
            className="chat-code-block__action"
            onClick={() => setWrapped((current) => !current)}
            title={t('files.wrapLines')}
            type="button"
          >
            <WrapText aria-hidden="true" size={15} strokeWidth={1.8} />
          </button>
          <button
            aria-label={copied ? t('chat.copied') : t('chat.copy')}
            className={`chat-code-block__action${copied ? ' is-copied' : ''}`}
            onClick={() => void handleCopy()}
            title={copied ? t('chat.copied') : t('chat.copy')}
            type="button"
          >
            {copied ? (
              <Check aria-hidden="true" size={15} strokeWidth={1.8} />
            ) : (
              <Copy aria-hidden="true" size={15} strokeWidth={1.8} />
            )}
          </button>
        </div>
      </div>
      <div className="chat-code-block__scroller">
        <pre>
          <code>
            {exactHighlightedLines ? (
              <HighlightedCode lines={exactHighlightedLines} />
            ) : highlightedPrefix ? (
              <>
                <HighlightedCode lines={highlightedPrefix.lines} />
                {code.slice(highlightedPrefix.code.length)}
              </>
            ) : (
              code
            )}
          </code>
        </pre>
      </div>
    </section>
  )
}

export const ChatCodeBlock = memo(
  ChatCodeBlockComponent,
  (previous, next) => previous.code === next.code && previous.language === next.language
)

ChatCodeBlock.displayName = 'ChatCodeBlock'
