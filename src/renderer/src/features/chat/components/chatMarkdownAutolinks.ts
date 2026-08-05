// Renderer chat Markdown: correct CJK prose boundaries in GFM-generated bare links.

interface MarkdownPoint {
  column?: number
  line?: number
  offset?: number
}

interface MarkdownPosition {
  end?: MarkdownPoint
  start?: MarkdownPoint
}

interface MarkdownNode {
  children?: unknown
  position?: MarkdownPosition
  title?: unknown
  type: string
  url?: unknown
  value?: unknown
}

const CJK_CHARACTER_SOURCE =
  '[\\p{Script=Han}\\p{Script=Hiragana}\\p{Script=Katakana}\\p{Script=Hangul}]'
const CJK_CHARACTER_PATTERN = new RegExp(CJK_CHARACTER_SOURCE, 'u')
const CJK_PROSE_BOUNDARY_PATTERN = new RegExp(
  `[，。；：！？、](?=$|${CJK_CHARACTER_SOURCE})|[,;:!?](?=${CJK_CHARACTER_SOURCE})`,
  'gu'
)

function isMarkdownNode(value: unknown): value is MarkdownNode {
  return Boolean(
    value && typeof value === 'object' && 'type' in value && typeof value.type === 'string'
  )
}

function pointsMatch(left: MarkdownPoint | undefined, right: MarkdownPoint | undefined): boolean {
  if (!left || !right) return false
  return left.line === right.line && left.column === right.column && left.offset === right.offset
}

function isGfmBareAutolink(node: MarkdownNode): boolean {
  if (node.type !== 'link' || typeof node.url !== 'string' || !Array.isArray(node.children)) {
    return false
  }

  const child = node.children[0]
  if (
    node.children.length !== 1 ||
    !isMarkdownNode(child) ||
    child.type !== 'text' ||
    typeof child.value !== 'string' ||
    child.value !== node.url
  ) {
    return false
  }

  // A GFM literal link and its text occupy exactly the same source range. Explicit
  // `[label](url)` and `<url>` links include delimiters and therefore fail this check.
  return (
    pointsMatch(node.position?.start, child.position?.start) &&
    pointsMatch(node.position?.end, child.position?.end)
  )
}

function urlPayloadBeforeBoundary(value: string, boundaryIndex: number): string {
  const prefix = value.slice(0, boundaryIndex)
  const schemeIndex = prefix.indexOf('://')
  if (schemeIndex === -1) return ''

  const authorityStart = schemeIndex + 3
  const relativePayloadIndex = prefix.slice(authorityStart).search(/[/?#]/)
  return relativePayloadIndex === -1 ? '' : prefix.slice(authorityStart + relativePayloadIndex)
}

function isValidHttpPrefix(value: string): boolean {
  try {
    const url = new URL(value)
    return Boolean(url.hostname) && (url.protocol === 'http:' || url.protocol === 'https:')
  } catch {
    return false
  }
}

export function splitCjkProseFromBareUrl(value: string): { suffix: string; url: string } | null {
  CJK_PROSE_BOUNDARY_PATTERN.lastIndex = 0
  let match: RegExpExecArray | null

  while ((match = CJK_PROSE_BOUNDARY_PATTERN.exec(value))) {
    const boundaryIndex = match.index
    const url = value.slice(0, boundaryIndex)
    if (!isValidHttpPrefix(url)) continue

    const suffix = value.slice(boundaryIndex)
    const payload = urlPayloadBeforeBoundary(value, boundaryIndex)

    // Raw punctuation can legitimately occur in a CJK path or query. Keep that
    // ambiguous case intact; users can still make the intended boundary explicit.
    if (CJK_CHARACTER_PATTERN.test(payload) && suffix.length > 1) continue

    return { suffix, url }
  }

  return null
}

function pointBeforeSuffix(
  point: MarkdownPoint | undefined,
  suffix: string
): MarkdownPoint | undefined {
  if (!point) return undefined
  return {
    ...point,
    column:
      typeof point.column === 'number' ? Math.max(1, point.column - suffix.length) : undefined,
    offset: typeof point.offset === 'number' ? Math.max(0, point.offset - suffix.length) : undefined
  }
}

function normalizeChildren(node: MarkdownNode): void {
  if (!Array.isArray(node.children)) return

  for (let index = 0; index < node.children.length; index += 1) {
    const child = node.children[index]
    if (!isMarkdownNode(child)) continue

    if (isGfmBareAutolink(child)) {
      const split = splitCjkProseFromBareUrl(child.url as string)
      const textChild = Array.isArray(child.children) ? child.children[0] : undefined

      if (split && isMarkdownNode(textChild)) {
        const originalEnd = child.position?.end
        const linkEnd = pointBeforeSuffix(originalEnd, split.suffix)

        child.url = split.url
        textChild.value = split.url
        if (child.position && linkEnd) child.position.end = linkEnd
        if (textChild.position && linkEnd) textChild.position.end = linkEnd

        node.children.splice(index + 1, 0, {
          type: 'text',
          value: split.suffix,
          position: {
            start: linkEnd,
            end: originalEnd
          }
        })
      }
    }

    normalizeChildren(child)
  }
}

export function remarkNormalizeCjkAutolinkBoundaries() {
  return (tree: unknown): void => {
    if (isMarkdownNode(tree)) normalizeChildren(tree)
  }
}
