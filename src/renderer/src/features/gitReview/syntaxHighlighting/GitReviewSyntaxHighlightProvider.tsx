import { createContext, useContext, useMemo, type CSSProperties, type ReactNode } from 'react'
import type { GitReviewFileContent } from '@mycopilot/protocol'
import type { GitDiffDocument } from '../diff'
import { buildGitReviewSyntaxSources } from '../syntax/buildGitReviewSyntaxSources'
import { resolveGitReviewFileLanguageDescriptor } from '../syntax/fileLanguageRegistry'
import {
  useGitReviewSyntaxHighlight,
  type GitReviewHighlightResult,
  type GitReviewHighlightToken,
  type GitReviewSyntaxHighlightState
} from './index'

export type GitReviewSyntaxSide = 'new' | 'old'

interface GitReviewSyntaxHighlightProviderProps {
  cacheKey: string
  children: ReactNode
  document: GitDiffDocument
  enabled: boolean
  fileContent?: GitReviewFileContent
  newPath: string
  oldPath: string
}

interface GitReviewSyntaxHighlightContextValue {
  newState: GitReviewSyntaxHighlightState
  oldState: GitReviewSyntaxHighlightState
}

interface GitReviewSyntaxCodeProps {
  content: string
  lineNumber?: number
  side: GitReviewSyntaxSide
}

const IDLE_HIGHLIGHT_STATE: GitReviewSyntaxHighlightState = { status: 'idle' }
const DEFAULT_CONTEXT: GitReviewSyntaxHighlightContextValue = {
  newState: IDLE_HIGHLIGHT_STATE,
  oldState: IDLE_HIGHLIGHT_STATE
}

const GitReviewSyntaxHighlightContext =
  createContext<GitReviewSyntaxHighlightContextValue>(DEFAULT_CONTEXT)

/**
 * Owns the two immutable source snapshots needed by a diff. The worker tokenizes old and new
 * independently so renames can also change grammar (for example, template.js -> template.ts).
 */
export function GitReviewSyntaxHighlightProvider({
  cacheKey,
  children,
  document,
  enabled,
  fileContent,
  newPath,
  oldPath
}: GitReviewSyntaxHighlightProviderProps): ReactNode {
  const sources = useMemo(
    () => buildGitReviewSyntaxSources(document, fileContent),
    [document, fileContent]
  )
  const languages = useMemo(
    () => resolveGitReviewFileLanguageDescriptor({ newPath, oldPath }),
    [newPath, oldPath]
  )
  const oldState = useGitReviewSyntaxHighlight({
    cacheKey: `${cacheKey}:old:${sources.oldSource.fidelity}:${oldPath}`,
    code: sources.oldSource.code,
    enabled: enabled && languages.oldLanguageId !== 'text' && sources.oldSource.code.length > 0,
    language: languages.oldLanguageId
  })
  const newState = useGitReviewSyntaxHighlight({
    cacheKey: `${cacheKey}:new:${sources.newSource.fidelity}:${newPath}`,
    code: sources.newSource.code,
    enabled: enabled && languages.newLanguageId !== 'text' && sources.newSource.code.length > 0,
    language: languages.newLanguageId
  })
  const value = useMemo(() => ({ newState, oldState }), [newState, oldState])

  return (
    <GitReviewSyntaxHighlightContext.Provider value={value}>
      {children}
    </GitReviewSyntaxHighlightContext.Provider>
  )
}

/** Paints trusted token foregrounds only; every non-ready state is byte-for-byte plain text. */
export function GitReviewSyntaxCode({
  content,
  lineNumber,
  side
}: GitReviewSyntaxCodeProps): ReactNode {
  const context = useContext(GitReviewSyntaxHighlightContext)
  const state = side === 'old' ? context.oldState : context.newState
  const tokens = resolveGitReviewSyntaxTokens(state, lineNumber, content)

  if (!tokens) {
    return <span className="git-review__diff-content">{content || ' '}</span>
  }

  return (
    <span className="git-review__diff-content" data-syntax-highlighted="true">
      {tokens.map((token) => (
        <span
          data-syntax-token="true"
          key={token.start}
          style={token.color ? ({ color: token.color } satisfies CSSProperties) : undefined}
        >
          {token.content}
        </span>
      ))}
    </span>
  )
}

/**
 * Resolves a diff line against a full side snapshot. The equality check is intentionally repeated
 * at paint time: a stale full-content response can never color the wrong patch line.
 */
export function resolveGitReviewSyntaxTokens(
  state: GitReviewSyntaxHighlightState,
  lineNumber: number | undefined,
  content: string
): readonly GitReviewHighlightToken[] | undefined {
  if (
    lineNumber === undefined ||
    lineNumber < 1 ||
    state.status !== 'ready' ||
    state.result.mode !== 'highlighted'
  ) {
    return undefined
  }

  return resolveLineTokens(state.result, lineNumber - 1, content)
}

function resolveLineTokens(
  result: GitReviewHighlightResult,
  sourceLineIndex: number,
  content: string
): readonly GitReviewHighlightToken[] | undefined {
  const line = result.lines[sourceLineIndex]
  if (!line || line.line !== sourceLineIndex || line.tokens.length === 0) return undefined

  let reconstructed = ''
  for (const token of line.tokens) reconstructed += token.content
  return reconstructed === content ? line.tokens : undefined
}
