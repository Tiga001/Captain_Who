import { fromMarkdown } from 'mdast-util-from-markdown'

const SUMMARY_MAX_CHARS = 520
const SUMMARY_MIN_CHARS = 60
const CJK_SUMMARY_MIN_CHARS = 32
const PRESENTATION_INPUT_MAX_CHARS = 40_000
const ARTICLE_TITLE_PATTERN =
  /(?:20\d{2}|招生|录取|报名|考试|简章|章程|公告|通知|办法|方案|计划|指南|名单|政策|admission|enrollment|application|announcement|notice|guide|policy|program)/i
const GENERIC_SITE_TITLE_PATTERN =
  /(?:官网|官方网站|首页|主页|网站首页|home|official site|welcome)$/i
const NAVIGATION_PREFIX_PATTERN =
  /^(?:当前位置|您所在的位置|首页|主页|网站首页|跳转到|导航|菜单|breadcrumb|home)(?:\s|[:：]|$)/i
const METADATA_PREFIX_PATTERN =
  /^(?:发布日期|发布时间|更新日期|更新时间|阅读次数|浏览次数|点击次数|来源|作者|编辑|published|updated|views?|source|author)(?:\s|[:：]|$)/i

type WebFetchSummaryQuality = 'good' | 'low'

export interface WebFetchPresentation {
  title: string
  summary?: string
  summaryQuality: WebFetchSummaryQuality
}

interface MarkdownNodeLike {
  type: string
  value?: string
  depth?: number
  children?: MarkdownNodeLike[]
}

interface MarkdownTextMetrics {
  text: string
  linkedChars: number
  imageCount: number
}

interface MarkdownBlock extends MarkdownTextMetrics {
  index: number
  type: string
  depth?: number
  rawText: string
}

interface TitleCandidate {
  index: number
  score: number
  title: string
}

function normalizeMarkdownStructure(content: string): string {
  return content
    .replace(/\r\n?/g, '\n')
    .replace(/[ \t]+(?=#{1,6}[ \t]+\S)/g, '\n')
    .replace(
      /[ \t]+(?=(?:发布日期|发布时间|更新日期|更新时间|阅读次数|浏览次数|点击次数|来源|作者|编辑|published|updated|views?|source|author)[ \t]*[:：])/gi,
      '\n'
    )
}

function collectNodeText(node: MarkdownNodeLike, insideLink = false): MarkdownTextMetrics {
  if (node.type === 'image' || node.type === 'imageReference' || node.type === 'definition') {
    return { text: '', linkedChars: 0, imageCount: 1 }
  }

  if (node.type === 'html') {
    return { text: '', linkedChars: 0, imageCount: 0 }
  }

  if (node.type === 'break') {
    return { text: ' ', linkedChars: 0, imageCount: 0 }
  }

  if (typeof node.value === 'string') {
    return {
      text: node.value,
      linkedChars: insideLink ? node.value.length : 0,
      imageCount: 0
    }
  }

  const isLink = insideLink || node.type === 'link' || node.type === 'linkReference'
  return (node.children ?? []).reduce<MarkdownTextMetrics>(
    (result, child) => {
      const childResult = collectNodeText(child, isLink)
      const separator = result.text && childResult.text && node.type !== 'paragraph' ? ' ' : ''
      result.text += `${separator}${childResult.text}`
      result.linkedChars += childResult.linkedChars
      result.imageCount += childResult.imageCount
      return result
    },
    { text: '', linkedChars: 0, imageCount: 0 }
  )
}

function cleanPlainText(value: string): string {
  return value
    .replace(/!\[[^\]]*\]\([^)]*\)/g, ' ')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\[[^\]]*\]/g, '$1')
    .replace(/<[^>]+>/g, ' ')
    .replace(/https?:\/\/[^\s<>(){}，。；！？]+/gi, ' ')
    .replace(/\/(?:[^\s/]+\/){2,}[^\s/]+\.(?:avif|gif|jpe?g|png|svg|webp)\b/gi, ' ')
    .replace(/[#*_`~]+/g, ' ')
    .replace(/\s*\|\s*/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

function extractMarkdownBlocks(content: string): MarkdownBlock[] {
  const tree = fromMarkdown(normalizeMarkdownStructure(content)) as MarkdownNodeLike

  return (tree.children ?? []).map((node, index) => {
    const metrics = collectNodeText(node)
    return {
      ...metrics,
      index,
      type: node.type,
      depth: node.depth,
      rawText: metrics.text,
      text: cleanPlainText(metrics.text)
    }
  })
}

function scoreTitle(block: MarkdownBlock): number {
  const title = block.text
  if (block.type !== 'heading' || title.length < 3 || title.length > 140) return -Infinity
  if (NAVIGATION_PREFIX_PATTERN.test(title) || METADATA_PREFIX_PATTERN.test(title)) return -Infinity

  let score = 2
  if (title.length >= 6 && title.length <= 80) score += 2
  if (/20\d{2}/.test(title)) score += 5
  if (ARTICLE_TITLE_PATTERN.test(title)) score += 5
  if ((block.depth ?? 1) > 1) score += 1
  if (GENERIC_SITE_TITLE_PATTERN.test(title)) score -= 10
  if (block.linkedChars / Math.max(block.rawText.length, 1) > 0.5) score -= 5
  if (title.length > 90 || /[。！？.!?]$/.test(title)) score -= 3
  return score
}

function selectTitle(
  blocks: MarkdownBlock[],
  fallbackTitle: string,
  providedTitle?: string
): TitleCandidate {
  const normalizedProvidedTitle = cleanPlainText(providedTitle ?? '')
  if (normalizedProvidedTitle && !/^https?:\/\//i.test(normalizedProvidedTitle)) {
    const matchingBlock = blocks.find((block) => block.text === normalizedProvidedTitle)
    return {
      index: matchingBlock?.index ?? -1,
      score: Infinity,
      title: normalizedProvidedTitle
    }
  }

  const candidates = blocks
    .map<TitleCandidate>((block) => ({
      index: block.index,
      score: scoreTitle(block),
      title: block.text
    }))
    .filter((candidate) => Number.isFinite(candidate.score))
    .sort((left, right) => right.score - left.score || left.index - right.index)

  const selected = candidates[0]
  if (selected && selected.score > 0) return selected

  return { index: -1, score: 0, title: fallbackTitle }
}

function stripLeadingMetadata(value: string): string {
  let result = value

  result = result.replace(
    /^(?:发布日期|发布时间|更新日期|更新时间|published|updated)\s*[:：]\s*\d{4}(?:[-/.年]\d{1,2}){1,2}(?:日)?(?:\s+\d{1,2}:\d{2})?\s*/i,
    ''
  )
  result = result.replace(/^(?:阅读次数|浏览次数|点击次数|views?)\s*[:：]\s*[\d,]+\s*/i, '')

  return result.trim()
}

function isTableLike(value: string): boolean {
  const sample = value.slice(0, 320)
  const pipeCount = (sample.match(/\|/g) ?? []).length
  return pipeCount >= 6 || /^\s*[-:]+(?:\s*\|\s*[-:]+){2,}/m.test(sample)
}

function stripTableTail(value: string): string {
  const firstPipe = value.indexOf('|')
  if (firstPipe < 0) return value

  const tailPipeCount = (value.slice(firstPipe).match(/\|/g) ?? []).length
  return tailPipeCount >= 4 ? value.slice(0, firstPipe).trimEnd() : value
}

function isMeaningfulSummaryBlock(block: MarkdownBlock, value: string, source: string): boolean {
  if (!value || value.length < 24) return false
  if (block.type === 'heading' || block.type === 'table' || block.type === 'code') return false
  if (NAVIGATION_PREFIX_PATTERN.test(value)) return false
  if (METADATA_PREFIX_PATTERN.test(value) && value.length < 100) return false
  if (isTableLike(source)) return false

  const rawLength = Math.max(block.rawText.length, 1)
  if (block.linkedChars / rawLength > 0.72 && value.length < 240) return false
  if (block.imageCount > 2 && value.length < 100) return false

  const meaningfulChars = (value.match(/[\p{L}\p{N}]/gu) ?? []).length
  return meaningfulChars >= 18 && meaningfulChars / value.length >= 0.45
}

function truncateSummary(value: string): string {
  if (value.length <= SUMMARY_MAX_CHARS) return value

  const slice = value.slice(0, SUMMARY_MAX_CHARS)
  const sentenceEnd = Math.max(
    slice.lastIndexOf('。'),
    slice.lastIndexOf('！'),
    slice.lastIndexOf('？'),
    slice.lastIndexOf('. '),
    slice.lastIndexOf('! '),
    slice.lastIndexOf('? '),
    slice.lastIndexOf('\n')
  )
  const end = sentenceEnd >= SUMMARY_MAX_CHARS * 0.6 ? sentenceEnd + 1 : SUMMARY_MAX_CHARS
  return `${slice.slice(0, end).trimEnd()}...`
}

function extractSummary(blocks: MarkdownBlock[], title: TitleCandidate): string | undefined {
  const startIndex = title.index >= 0 ? title.index + 1 : 0
  const parts: string[] = []

  for (const block of blocks) {
    if (block.index < startIndex) continue
    const source = stripTableTail(block.rawText)
    const value = stripLeadingMetadata(cleanPlainText(source))
    if (!isMeaningfulSummaryBlock(block, value, source)) continue
    parts.push(value)
    if (parts.join('\n').length >= SUMMARY_MAX_CHARS) break
  }

  const summary = truncateSummary(parts.join('\n'))
  const meaningfulChars = (summary.match(/[\p{L}\p{N}]/gu) ?? []).length
  const containsCjk =
    /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u.test(summary)
  const minimumLength = containsCjk ? CJK_SUMMARY_MIN_CHARS : SUMMARY_MIN_CHARS
  const minimumMeaningfulChars = containsCjk ? 24 : 40
  const isGoodQuality =
    summary.length >= minimumLength &&
    meaningfulChars >= minimumMeaningfulChars &&
    meaningfulChars / Math.max(summary.length, 1) >= 0.45

  return isGoodQuality ? summary : undefined
}

export function buildWebFetchPresentation(
  content: string,
  fallbackTitle: string,
  providedTitle?: string
): WebFetchPresentation {
  if (!content.trim()) {
    return { title: fallbackTitle, summaryQuality: 'low' }
  }

  try {
    const blocks = extractMarkdownBlocks(content.slice(0, PRESENTATION_INPUT_MAX_CHARS))
    const title = selectTitle(blocks, fallbackTitle, providedTitle)
    const summary = extractSummary(blocks, title)
    return {
      title: title.title || fallbackTitle,
      summary,
      summaryQuality: summary ? 'good' : 'low'
    }
  } catch {
    return { title: fallbackTitle, summaryQuality: 'low' }
  }
}
