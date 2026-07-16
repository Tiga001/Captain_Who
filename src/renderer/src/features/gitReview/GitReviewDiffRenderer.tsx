import type { GitReviewFile, GitReviewFileContent } from '@mycopilot/protocol'
import { useMemo, useRef, type ReactNode, type RefObject, type UIEventHandler } from 'react'
import { AlertCircle, LoaderCircle, RefreshCw } from 'lucide-react'
import type { Translate } from '../../config/translationFormat'
import {
  buildGitDiffDocument,
  buildSplitDiffBlocks,
  getGitDiffExpandedSlices,
  hydrateGitDiffDocument,
  parseGitPatch,
  type GitDiffDocument,
  type GitDiffExpansionAction,
  type GitDiffExpansionDirection,
  type GitDiffExpansionState,
  type GitDiffGapSection,
  type GitDiffHunk,
  type GitDiffLine,
  type GitDiffSection,
  type GitSplitDiffBlock,
  type GitSplitDiffRow
} from './diff'
import { GitReviewGapSeparator } from './GitReviewGapSeparator'
import { GitReviewSplitBuffer } from './GitReviewSplitBuffer'
import { getGitReviewFileShape, resolveGitReviewDiffLayout } from './gitReviewDiffLayout'
import type { GitReviewViewMode } from './gitReviewViewMode'
import { useLinkedDiffScroll } from './useLinkedDiffScroll'
import type { GitReviewDiffState, GitReviewFileContentState } from './useGitReview'
import {
  GitReviewSyntaxCode,
  GitReviewSyntaxHighlightProvider,
  type GitReviewSyntaxSide
} from './syntaxHighlighting/GitReviewSyntaxHighlightProvider'

export type GitReviewDiffExpandHandler = (
  action: GitDiffExpansionAction,
  scrollAnchorId?: string
) => void

interface GitReviewDiffRendererProps {
  diffState?: GitReviewDiffState
  expansionState?: GitDiffExpansionState
  file: GitReviewFile
  fileContentState?: GitReviewFileContentState
  onExpand?: GitReviewDiffExpandHandler
  onRequestDiff: (fileId: string) => void
  syntaxHighlightingEnabled?: boolean
  t: Translate
  viewMode: GitReviewViewMode
  wrapLines: boolean
}

/** Maps request state to the pure document renderer; full-content failures never replace a diff. */
export function GitReviewDiffRenderer({
  diffState,
  expansionState,
  file,
  fileContentState,
  onExpand,
  onRequestDiff,
  syntaxHighlightingEnabled = true,
  t,
  viewMode,
  wrapLines
}: GitReviewDiffRendererProps): ReactNode {
  if (!diffState || diffState.status === 'idle' || diffState.status === 'loading') {
    return (
      <div className="git-review__diff-message">
        <LoaderCircle className="git-review__spinner" aria-hidden="true" />
        <span>{t('gitReview.diff.loading')}</span>
      </div>
    )
  }

  if (diffState.status === 'error') {
    return (
      <div className="git-review__diff-message git-review__diff-message--error" role="alert">
        <AlertCircle aria-hidden="true" />
        <span>{diffState.error}</span>
        <button type="button" onClick={() => onRequestDiff(file.id)}>
          <RefreshCw aria-hidden="true" />
          {t('gitReview.retry')}
        </button>
      </div>
    )
  }

  if (diffState.value.status === 'binary') {
    return <div className="git-review__diff-message">{t('gitReview.diff.binary')}</div>
  }
  if (diffState.value.status === 'tooLarge') {
    return <div className="git-review__diff-message">{t('gitReview.diff.tooLarge')}</div>
  }

  return (
    <GitPatchRenderer
      emptyState={t('gitReview.diff.noHunks')}
      expansionState={expansionState}
      file={file}
      fileContent={fileContentState?.status === 'ready' ? fileContentState.value : undefined}
      invalidState={t('gitReview.diff.invalid')}
      onExpand={onExpand}
      patch={diffState.value.patch ?? ''}
      snapshotId={diffState.value.snapshotId}
      syntaxHighlightingEnabled={syntaxHighlightingEnabled}
      t={t}
      viewMode={viewMode}
      wrapLines={wrapLines}
    />
  )
}

interface GitPatchRendererProps {
  emptyState: ReactNode
  expansionState?: GitDiffExpansionState
  file: GitReviewFile
  fileContent?: GitReviewFileContent
  invalidState: ReactNode
  onExpand?: GitReviewDiffExpandHandler
  patch: string
  snapshotId: string
  syntaxHighlightingEnabled?: boolean
  t: Translate
  viewMode: GitReviewViewMode
  wrapLines: boolean
}

/** Pure parse → validate → paint boundary; transport, caching and mutation stay above it. */
export function GitPatchRenderer({
  emptyState,
  expansionState = EMPTY_EXPANSION_STATE,
  file,
  fileContent,
  invalidState,
  onExpand,
  patch,
  snapshotId,
  syntaxHighlightingEnabled = true,
  t,
  viewMode,
  wrapLines
}: GitPatchRendererProps): ReactNode {
  const renderModel = useMemo(() => {
    const parsed = parseGitPatch(patch)
    if (!parsed.ok) return { documentResult: parsed }
    const compact = buildGitDiffDocument(parsed.value)
    if (!compact.ok) return { documentResult: compact }
    if (fileContent?.status !== 'ready') return { documentResult: compact }
    if (fileContent.beforeText === null || fileContent.afterText === null) {
      // Added/deleted files only have one side; paint-time equality still guards that snapshot.
      return { documentResult: compact, syntaxFileContent: fileContent }
    }
    const hydrated = hydrateGitDiffDocument(compact.value, {
      newText: fileContent.afterText,
      oldText: fileContent.beforeText
    })
    // A stale or inconsistent full-content response must never corrupt the compact diff.
    return hydrated.ok
      ? { documentResult: hydrated, syntaxFileContent: fileContent }
      : { documentResult: compact }
  }, [fileContent, patch])
  const { documentResult } = renderModel
  const layout = resolveGitReviewDiffLayout(viewMode, getGitReviewFileShape(file.status))

  if (!documentResult.ok) {
    return <div className="git-review__diff-message">{invalidState}</div>
  }
  if (documentResult.value.hunks.length === 0) {
    return <div className="git-review__diff-message">{emptyState}</div>
  }

  const sharedProps: DiffDocumentProps = {
    document: documentResult.value,
    expansionState,
    onExpand,
    t,
    wrapLines
  }
  const diff =
    layout === 'unified' ? (
      <UnifiedDiff {...sharedProps} />
    ) : layout === 'single-old' || layout === 'single-new' ? (
      <SingleSidedDiff {...sharedProps} side={layout === 'single-old' ? 'old' : 'new'} />
    ) : (
      <SplitDiff {...sharedProps} />
    )

  return (
    <GitReviewSyntaxHighlightProvider
      cacheKey={`${snapshotId}:${file.id}`}
      document={documentResult.value}
      enabled={syntaxHighlightingEnabled}
      fileContent={renderModel.syntaxFileContent}
      newPath={file.path}
      oldPath={file.previousPath ?? file.path}
    >
      {diff}
    </GitReviewSyntaxHighlightProvider>
  )
}

const EMPTY_EXPANSION_STATE: GitDiffExpansionState = new Map()

interface DiffDocumentProps {
  document: GitDiffDocument
  expansionState: GitDiffExpansionState
  onExpand?: GitReviewDiffExpandHandler
  t: Translate
  wrapLines: boolean
}

interface GapRenderPlan {
  collapsedCount: number
  endLines: readonly GitDiffLine[]
  startLines: readonly GitDiffLine[]
}

interface SplitScrollableHunkSection {
  blocks: readonly GitSplitDiffBlock[]
  id: string
  kind: 'hunk'
}

interface SplitScrollableGapSection {
  gap: GitDiffGapSection
  id: string
  kind: 'gap'
  nextHunkAnchorId?: string
  plan: GapRenderPlan
}

type SplitScrollableSection = SplitScrollableHunkSection | SplitScrollableGapSection

function buildGapRenderPlan(
  gap: GitDiffGapSection,
  expansionState: GitDiffExpansionState
): GapRenderPlan {
  if (!gap.lines) {
    return { collapsedCount: gap.lineCount, endLines: [], startLines: [] }
  }
  const slices = getGitDiffExpandedSlices(gap.lineCount, expansionState.get(gap.id))
  return {
    collapsedCount: slices.collapsedCount,
    endLines: slices.end ? gap.lines.slice(slices.end.start, slices.end.end) : [],
    startLines: slices.start ? gap.lines.slice(slices.start.start, slices.start.end) : []
  }
}

function findNextHunkAnchorId(
  sections: readonly GitDiffSection[],
  index: number
): string | undefined {
  for (let nextIndex = index + 1; nextIndex < sections.length; nextIndex += 1) {
    const section = sections[nextIndex]
    if (section?.kind === 'hunk') return section.id
  }
  return undefined
}

/** A single semantic row plan is projected into both native horizontal scrollports. */
function buildSplitScrollablePlan(
  document: GitDiffDocument,
  expansionState: GitDiffExpansionState
): readonly SplitScrollableSection[] {
  return document.sections.map((section, sectionIndex) => {
    if (section.kind === 'hunk') {
      return {
        blocks: buildSplitDiffBlocks(section.hunk),
        id: section.id,
        kind: 'hunk'
      }
    }
    return {
      gap: section,
      id: section.id,
      kind: 'gap',
      nextHunkAnchorId: findNextHunkAnchorId(document.sections, sectionIndex),
      plan: buildGapRenderPlan(section, expansionState)
    }
  })
}

function requestGapExpansion(
  gap: GitDiffGapSection,
  direction: GitDiffExpansionDirection,
  nextHunkAnchorId: string | undefined,
  onExpand: GitReviewDiffExpandHandler | undefined
): void {
  onExpand?.(
    { direction, gapId: gap.id, lineCount: gap.lineCount, type: 'expand' },
    direction === 'up' || direction === 'both' ? nextHunkAnchorId : undefined
  )
}

function UnifiedDiff({
  document,
  expansionState,
  onExpand,
  t,
  wrapLines
}: DiffDocumentProps): ReactNode {
  return (
    <div className="git-review__unified-diff" data-wrap={wrapLines ? 'true' : undefined}>
      {document.sections.map((section, sectionIndex) => {
        if (section.kind === 'hunk') {
          return (
            <div
              className="git-review__hunk"
              data-diff-anchor-id={section.id}
              data-primary-anchor="true"
              key={section.id}
            >
              {section.hunk.lines.map((line, lineIndex) => (
                <UnifiedDiffLine key={`${section.id}:line:${lineIndex}`} line={line} />
              ))}
            </div>
          )
        }
        const plan = buildGapRenderPlan(section, expansionState)
        const nextAnchorId = findNextHunkAnchorId(document.sections, sectionIndex)
        return (
          <div className="git-review__gap-block" data-gap-id={section.id} key={section.id}>
            {plan.startLines.map((line, index) => (
              <UnifiedDiffLine key={`${section.id}:start:${index}`} line={line} />
            ))}
            {plan.collapsedCount > 0 && (
              <GitReviewGapSeparator
                lineCount={plan.collapsedCount}
                onExpand={
                  section.lines && onExpand
                    ? (direction) => requestGapExpansion(section, direction, nextAnchorId, onExpand)
                    : undefined
                }
                position={section.position}
                t={t}
              />
            )}
            {plan.endLines.map((line, index) => (
              <UnifiedDiffLine key={`${section.id}:end:${index}`} line={line} />
            ))}
          </div>
        )
      })}
    </div>
  )
}

function UnifiedDiffLine({ line }: { line: GitDiffLine }): ReactNode {
  return (
    <div className={`git-review__unified-line git-review__diff-line--${line.kind}`}>
      <span className="git-review__line-number">{line.oldLineNumber ?? ''}</span>
      <span className="git-review__line-number">{line.newLineNumber ?? ''}</span>
      <DiffCode line={line} side={line.kind === 'deletion' ? 'old' : 'new'} />
    </div>
  )
}

function SplitDiff(props: DiffDocumentProps): ReactNode {
  return props.wrapLines ? <SplitWrappedDiff {...props} /> : <SplitScrollableDiff {...props} />
}

function SplitWrappedDiff({ document, expansionState, onExpand, t }: DiffDocumentProps): ReactNode {
  return (
    <div className="git-review__split-diff git-review__split-diff--wrapped" data-wrap="true">
      {document.sections.map((section, sectionIndex) => {
        if (section.kind === 'hunk') {
          return (
            <div
              className="git-review__hunk"
              data-diff-anchor-id={section.id}
              data-primary-anchor="true"
              key={section.id}
            >
              <SplitWrappedHunk hunk={section.hunk} />
            </div>
          )
        }
        const plan = buildGapRenderPlan(section, expansionState)
        const nextAnchorId = findNextHunkAnchorId(document.sections, sectionIndex)
        return (
          <div className="git-review__gap-block" data-gap-id={section.id} key={section.id}>
            {plan.startLines.map((line, index) => (
              <SplitDiffRow
                key={`${section.id}:start:${index}`}
                row={{ left: line, right: line }}
              />
            ))}
            {plan.collapsedCount > 0 && (
              <GitReviewGapSeparator
                lineCount={plan.collapsedCount}
                onExpand={
                  section.lines && onExpand
                    ? (direction) => requestGapExpansion(section, direction, nextAnchorId, onExpand)
                    : undefined
                }
                position={section.position}
                t={t}
              />
            )}
            {plan.endLines.map((line, index) => (
              <SplitDiffRow key={`${section.id}:end:${index}`} row={{ left: line, right: line }} />
            ))}
          </div>
        )
      })}
    </div>
  )
}

function SplitWrappedHunk({ hunk }: { hunk: GitDiffHunk }): ReactNode {
  return buildSplitDiffBlocks(hunk).map((block) => {
    if (block.kind === 'paired') {
      return block.rows.map((row, index) => (
        <SplitDiffRow key={`${block.id}:row:${index}`} row={row} />
      ))
    }
    return <SplitWrappedOneSidedBlock block={block} key={block.id} />
  })
}

function SplitWrappedOneSidedBlock({
  block
}: {
  block: Extract<GitSplitDiffBlock, { kind: 'one-sided' }>
}): ReactNode {
  const presentLines = (
    <div className="git-review__split-block-lines">
      {block.lines.map((line, index) => (
        <SplitDiffSide key={`${block.id}:line:${index}`} line={line} side={block.presentSide} />
      ))}
    </div>
  )
  return (
    <div className="git-review__split-one-sided-block" data-present-side={block.presentSide}>
      {block.presentSide === 'left' ? (
        presentLines
      ) : (
        <GitReviewSplitBuffer lineCount={block.rowSpan} />
      )}
      {block.presentSide === 'right' ? (
        presentLines
      ) : (
        <GitReviewSplitBuffer lineCount={block.rowSpan} />
      )}
    </div>
  )
}

function SplitScrollableDiff({
  document,
  expansionState,
  onExpand,
  t
}: DiffDocumentProps): ReactNode {
  const leftCanvasRef = useRef<HTMLDivElement>(null)
  const leftContentRef = useRef<HTMLDivElement>(null)
  const leftPaneRef = useRef<HTMLDivElement>(null)
  const rightCanvasRef = useRef<HTMLDivElement>(null)
  const rightContentRef = useRef<HTMLDivElement>(null)
  const rightPaneRef = useRef<HTMLDivElement>(null)
  const sections = useMemo(
    () => buildSplitScrollablePlan(document, expansionState),
    [document, expansionState]
  )
  const { onLeftScroll, onRightScroll } = useLinkedDiffScroll({
    leftCanvasRef,
    leftContentRef,
    leftPaneRef,
    rightCanvasRef,
    rightContentRef,
    rightPaneRef
  })

  return (
    <div className="git-review__split-diff git-review__split-diff--scrollable">
      <SplitDiffPane
        canvasRef={leftCanvasRef}
        contentRef={leftContentRef}
        onExpand={onExpand}
        onScroll={onLeftScroll}
        paneRef={leftPaneRef}
        sections={sections}
        side="left"
        t={t}
      />
      <SplitDiffPane
        canvasRef={rightCanvasRef}
        contentRef={rightContentRef}
        onExpand={onExpand}
        onScroll={onRightScroll}
        paneRef={rightPaneRef}
        sections={sections}
        side="right"
        t={t}
      />
    </div>
  )
}

interface SplitDiffPaneProps {
  canvasRef: RefObject<HTMLDivElement | null>
  contentRef: RefObject<HTMLDivElement | null>
  onExpand?: GitReviewDiffExpandHandler
  onScroll: UIEventHandler<HTMLDivElement>
  paneRef: RefObject<HTMLDivElement | null>
  sections: readonly SplitScrollableSection[]
  side: 'left' | 'right'
  t: Translate
}

function SplitDiffPane({
  canvasRef,
  contentRef,
  onExpand,
  onScroll,
  paneRef,
  sections,
  side,
  t
}: SplitDiffPaneProps): ReactNode {
  const semanticSide = side === 'left' ? 'old' : 'new'
  return (
    <div
      className="git-review__split-pane"
      data-diff-pane={semanticSide}
      onScroll={onScroll}
      ref={paneRef}
    >
      <div className="git-review__split-pane-canvas" ref={canvasRef}>
        <div className="git-review__split-pane-content" ref={contentRef}>
          {sections.map((section) => {
            if (section.kind === 'hunk') {
              return (
                <div
                  className="git-review__hunk"
                  data-diff-anchor-id={section.id}
                  data-primary-anchor={side === 'left' ? 'true' : undefined}
                  key={section.id}
                >
                  <SplitPaneHunk blocks={section.blocks} side={side} />
                </div>
              )
            }
            const { gap, plan } = section
            return (
              <div className="git-review__gap-block" data-gap-id={section.id} key={section.id}>
                {plan.startLines.map((line, index) => (
                  <SplitPaneLine key={`${section.id}:start:${index}`} line={line} side={side} />
                ))}
                {plan.collapsedCount > 0 && (
                  <GitReviewGapSeparator
                    fragment={side === 'left' ? 'left' : 'right'}
                    lineCount={plan.collapsedCount}
                    onExpand={
                      side === 'left' && gap.lines && onExpand
                        ? (direction) =>
                            requestGapExpansion(gap, direction, section.nextHunkAnchorId, onExpand)
                        : undefined
                    }
                    position={gap.position}
                    t={t}
                  />
                )}
                {plan.endLines.map((line, index) => (
                  <SplitPaneLine key={`${section.id}:end:${index}`} line={line} side={side} />
                ))}
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
}

function SplitPaneHunk({
  blocks,
  side
}: {
  blocks: readonly GitSplitDiffBlock[]
  side: 'left' | 'right'
}): ReactNode {
  return blocks.map((block) => {
    if (block.kind === 'paired') {
      return block.rows.map((row, index) => (
        <SplitPaneLine
          key={`${block.id}:row:${index}`}
          line={side === 'left' ? row.left : row.right}
          side={side}
        />
      ))
    }
    if (block.presentSide === side) {
      return block.lines.map((line, index) => (
        <SplitPaneLine key={`${block.id}:line:${index}`} line={line} side={side} />
      ))
    }
    return <GitReviewSplitBuffer key={block.id} lineCount={block.rowSpan} />
  })
}

interface SplitDiffRowProps {
  row: GitSplitDiffRow
}

function SplitDiffRow({ row }: SplitDiffRowProps): ReactNode {
  return (
    <div className="git-review__split-row">
      <SplitDiffSide line={row.left} side="left" />
      <SplitDiffSide line={row.right} side="right" />
    </div>
  )
}

interface SplitDiffSideProps {
  line: GitDiffLine
  side: 'left' | 'right'
}

function SplitDiffSide({ line, side }: SplitDiffSideProps): ReactNode {
  const lineNumber = side === 'left' ? line.oldLineNumber : line.newLineNumber
  return (
    <div className={`git-review__split-side git-review__diff-line--${line.kind}`} data-side={side}>
      <span className="git-review__line-number">{lineNumber ?? ''}</span>
      <DiffCode line={line} side={side === 'left' ? 'old' : 'new'} />
    </div>
  )
}

function SplitPaneLine({ line, side }: SplitDiffSideProps): ReactNode {
  const lineNumber = side === 'left' ? line.oldLineNumber : line.newLineNumber
  return (
    <div className={`git-review__split-pane-line git-review__diff-line--${line.kind}`}>
      <span className="git-review__line-number">{lineNumber ?? ''}</span>
      <DiffCode line={line} side={side === 'left' ? 'old' : 'new'} />
    </div>
  )
}

type SingleDiffSide = 'old' | 'new'

interface SingleSidedDiffProps extends DiffDocumentProps {
  side: SingleDiffSide
}

function SingleSidedDiff({
  document,
  expansionState,
  onExpand,
  side,
  t,
  wrapLines
}: SingleSidedDiffProps): ReactNode {
  return (
    <div
      className="git-review__single-diff"
      data-side={side}
      data-wrap={wrapLines ? 'true' : undefined}
    >
      <div className="git-review__single-diff-content" data-diff-pane={side}>
        {document.sections.map((section, sectionIndex) => {
          if (section.kind === 'hunk') {
            return (
              <div
                className="git-review__hunk"
                data-diff-anchor-id={section.id}
                data-primary-anchor="true"
                key={section.id}
              >
                {section.hunk.lines
                  .filter((line) => isLineVisibleOnSide(line, side))
                  .map((line, lineIndex) => (
                    <SingleDiffLine
                      key={`${section.id}:line:${lineIndex}`}
                      line={line}
                      side={side}
                    />
                  ))}
              </div>
            )
          }
          const plan = buildGapRenderPlan(section, expansionState)
          const nextAnchorId = findNextHunkAnchorId(document.sections, sectionIndex)
          return (
            <div className="git-review__gap-block" data-gap-id={section.id} key={section.id}>
              {plan.startLines.map((line, index) => (
                <SingleDiffLine key={`${section.id}:start:${index}`} line={line} side={side} />
              ))}
              {plan.collapsedCount > 0 && (
                <GitReviewGapSeparator
                  lineCount={plan.collapsedCount}
                  onExpand={
                    section.lines && onExpand
                      ? (direction) =>
                          requestGapExpansion(section, direction, nextAnchorId, onExpand)
                      : undefined
                  }
                  position={section.position}
                  t={t}
                />
              )}
              {plan.endLines.map((line, index) => (
                <SingleDiffLine key={`${section.id}:end:${index}`} line={line} side={side} />
              ))}
            </div>
          )
        })}
      </div>
    </div>
  )
}

function isLineVisibleOnSide(line: GitDiffLine, side: SingleDiffSide): boolean {
  if (line.kind === 'context' || line.kind === 'meta') return true
  return side === 'old' ? line.kind === 'deletion' : line.kind === 'addition'
}

function SingleDiffLine({ line, side }: { line: GitDiffLine; side: SingleDiffSide }): ReactNode {
  const lineNumber = side === 'old' ? line.oldLineNumber : line.newLineNumber
  return (
    <div className={`git-review__single-line git-review__diff-line--${line.kind}`}>
      <span className="git-review__line-number">{lineNumber ?? ''}</span>
      <DiffCode line={line} side={side} />
    </div>
  )
}

function DiffCode({ line, side }: { line: GitDiffLine; side: GitReviewSyntaxSide }): ReactNode {
  const prefix =
    line.kind === 'addition'
      ? '+'
      : line.kind === 'deletion'
        ? '-'
        : line.kind === 'meta'
          ? ''
          : ' '
  return (
    <span className="git-review__diff-code">
      <span className="git-review__diff-prefix" aria-hidden="true">
        {prefix}
      </span>
      <GitReviewSyntaxCode
        content={line.content}
        lineNumber={side === 'old' ? line.oldLineNumber : line.newLineNumber}
        side={side}
      />
    </span>
  )
}
