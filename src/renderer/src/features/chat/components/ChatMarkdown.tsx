import ReactMarkdown from 'react-markdown'
import rehypeKatex from 'rehype-katex'
import remarkBreaks from 'remark-breaks'
import remarkGfm from 'remark-gfm'
import remarkMath from 'remark-math'
import 'katex/dist/katex.min.css'
import { openExternalUrl } from '../../../lib/externalLinks'
import { remarkNormalizeCjkAutolinkBoundaries } from './chatMarkdownAutolinks'
import { useImagePreview } from './ImagePreview'

interface ChatMarkdownProps {
  className?: string
  content: string
}

interface MarkdownLine {
  body: string
  eol: string
}

function splitMarkdownLines(value: string): MarkdownLine[] {
  const lines: MarkdownLine[] = []
  const linePattern = /([^\r\n]*)(\r\n|\n|\r|$)/g
  let match: RegExpExecArray | null

  while ((match = linePattern.exec(value))) {
    if (match[0] === '') break
    lines.push({ body: match[1], eol: match[2] })
  }

  return lines
}

function getFenceMarker(line: string) {
  const match = /^( {0,3})(`{3,}|~{3,})/.exec(line)
  return match?.[2]
}

function isClosingFence(line: string, openingMarker: string) {
  const markerCharacter = openingMarker[0]
  const markerLength = openingMarker.length
  const pattern = new RegExp(`^ {0,3}${markerCharacter}{${markerLength},}\\s*$`)
  return pattern.test(line)
}

function transformInlineCodeAware(value: string, transform: (segment: string) => string) {
  let result = ''
  let index = 0

  while (index < value.length) {
    const nextBacktickIndex = value.indexOf('`', index)

    if (nextBacktickIndex === -1) {
      result += transform(value.slice(index))
      break
    }

    result += transform(value.slice(index, nextBacktickIndex))

    let tickEndIndex = nextBacktickIndex + 1
    while (tickEndIndex < value.length && value[tickEndIndex] === '`') {
      tickEndIndex += 1
    }

    const tickMarker = value.slice(nextBacktickIndex, tickEndIndex)
    const closingIndex = value.indexOf(tickMarker, tickEndIndex)

    if (closingIndex === -1) {
      result += value.slice(nextBacktickIndex)
      break
    }

    result += value.slice(nextBacktickIndex, closingIndex + tickMarker.length)
    index = closingIndex + tickMarker.length
  }

  return result
}

function transformFencedCodeAware(value: string, transform: (segment: string) => string) {
  const lines = splitMarkdownLines(value)
  let result = ''
  let pendingText = ''
  let openingFenceMarker: string | undefined

  const flushPendingText = () => {
    if (!pendingText) return
    result += transformInlineCodeAware(pendingText, transform)
    pendingText = ''
  }

  for (const line of lines) {
    const rawLine = `${line.body}${line.eol}`

    if (openingFenceMarker) {
      result += rawLine
      if (isClosingFence(line.body, openingFenceMarker)) {
        openingFenceMarker = undefined
      }
      continue
    }

    const nextFenceMarker = getFenceMarker(line.body)
    if (nextFenceMarker) {
      flushPendingText()
      openingFenceMarker = nextFenceMarker
      result += rawLine
      continue
    }

    pendingText += rawLine
  }

  flushPendingText()
  return result
}

function hasMathSyntax(value: string) {
  return /\\[a-zA-Z]+|[_^=<>≤≥≈]|[∑∫√∞]|\\(?:to|approx|le|ge|times|cdot)/.test(value)
}

function isSimpleMathExpression(value: string) {
  const token = '(?:\\d+(?:\\.\\d+)?|[A-Za-z])'
  return new RegExp(`^${token}(?:\\s*[+\\-*/]\\s*${token})+$`).test(value)
}

function shouldNormalizeBareParentheticalMath(value: string) {
  const expression = value.trim()
  if (!expression || expression.length > 240) return false
  if (/[。！？；，、]/.test(expression)) return false
  if (/^[A-Za-z]$/.test(expression)) return true
  if (hasMathSyntax(expression)) return true
  return isSimpleMathExpression(expression)
}

function normalizeBareParentheticalMath(value: string) {
  return value.replace(
    /\(([^()\n]+)\)/g,
    (match, expression: string, offset: number, source: string) => {
      if (source[offset - 1] === ']') return match
      if (!shouldNormalizeBareParentheticalMath(expression)) return match
      return `$${expression.trim()}$`
    }
  )
}

function shouldNormalizeBracketMathBlock(
  lines: MarkdownLine[],
  startIndex: number,
  endIndex: number
) {
  const blockContent = lines
    .slice(startIndex + 1, endIndex)
    .map((line) => line.body)
    .join('\n')
    .trim()

  return Boolean(blockContent) && hasMathSyntax(blockContent)
}

function normalizeBracketMathBlocks(value: string) {
  const lines = splitMarkdownLines(value)
  let result = ''
  let index = 0

  while (index < lines.length) {
    const line = lines[index]

    if (line.body.trim() !== '[') {
      result += `${line.body}${line.eol}`
      index += 1
      continue
    }

    const closingIndex = lines.findIndex((candidate, candidateIndex) => {
      return candidateIndex > index && candidate.body.trim() === ']'
    })

    if (closingIndex === -1 || !shouldNormalizeBracketMathBlock(lines, index, closingIndex)) {
      result += `${line.body}${line.eol}`
      index += 1
      continue
    }

    result += `$$${line.eol}`
    for (let blockLineIndex = index + 1; blockLineIndex < closingIndex; blockLineIndex += 1) {
      const blockLine = lines[blockLineIndex]
      result += `${blockLine.body}${blockLine.eol}`
    }
    result += `$$${lines[closingIndex].eol}`
    index = closingIndex + 1
  }

  return result
}

function isEscaped(value: string, index: number) {
  let backslashCount = 0
  for (let cursor = index - 1; cursor >= 0 && value[cursor] === '\\'; cursor -= 1) {
    backslashCount += 1
  }
  return backslashCount % 2 === 1
}

function findNextUnescaped(value: string, search: string, fromIndex: number) {
  let matchIndex = value.indexOf(search, fromIndex)
  while (matchIndex !== -1 && isEscaped(value, matchIndex)) {
    matchIndex = value.indexOf(search, matchIndex + search.length)
  }
  return matchIndex
}

function transformDollarMathAware(value: string, transform: (segment: string) => string) {
  let result = ''
  let index = 0

  while (index < value.length) {
    const nextDollarIndex = findNextUnescaped(value, '$', index)

    if (nextDollarIndex === -1) {
      result += transform(value.slice(index))
      break
    }

    result += transform(value.slice(index, nextDollarIndex))

    const marker = value.startsWith('$$', nextDollarIndex) ? '$$' : '$'
    const closingIndex = findNextUnescaped(value, marker, nextDollarIndex + marker.length)

    if (closingIndex === -1) {
      result += transform(value.slice(nextDollarIndex))
      break
    }

    result += value.slice(nextDollarIndex, closingIndex + marker.length)
    index = closingIndex + marker.length
  }

  return result
}

function findMatchingLatexBrace(value: string, openBraceIndex: number) {
  if (value[openBraceIndex] !== '{') return -1

  let depth = 0

  for (let index = openBraceIndex; index < value.length; index += 1) {
    if (isEscaped(value, index)) continue

    if (value[index] === '{') {
      depth += 1
    } else if (value[index] === '}') {
      depth -= 1
      if (depth === 0) return index
    }
  }

  return -1
}

function unwrapLatexBoxCommands(value: string): string {
  const boxCommandPattern = /\\(?:boxed|fbox)\s*\{/g
  let result = ''
  let index = 0

  while (index < value.length) {
    boxCommandPattern.lastIndex = index
    const match = boxCommandPattern.exec(value)

    if (!match) {
      result += value.slice(index)
      break
    }

    const commandIndex = match.index
    const openBraceIndex = boxCommandPattern.lastIndex - 1

    if (isEscaped(value, commandIndex)) {
      result += value.slice(index, commandIndex + 1)
      index = commandIndex + 1
      continue
    }

    const closeBraceIndex = findMatchingLatexBrace(value, openBraceIndex)

    if (closeBraceIndex === -1) {
      result += value.slice(index)
      break
    }

    result += value.slice(index, commandIndex)
    result += unwrapLatexBoxCommands(value.slice(openBraceIndex + 1, closeBraceIndex))
    index = closeBraceIndex + 1
  }

  return result
}

function normalizeLatexDelimiters(value: string) {
  return value
    .replace(/\\\[([\s\S]+?)\\\]/g, (_, expression: string) => `$$\n${expression}\n$$`)
    .replace(/\\\(([\s\S]+?)\\\)/g, (_, expression: string) => `$${expression}$`)
}

function normalizeMarkdownMath(content: string) {
  return transformFencedCodeAware(content, (segment) => {
    const normalizedBoxes = unwrapLatexBoxCommands(normalizeBracketMathBlocks(segment))
    const normalizedDelimitedMath = normalizeLatexDelimiters(normalizedBoxes)
    return transformDollarMathAware(normalizedDelimitedMath, normalizeBareParentheticalMath)
  })
}

function normalizeMarkdownExternalHref(href: string | undefined) {
  if (!href) return null

  const trimmedHref = href.trim()
  if (!trimmedHref) return null

  let normalizedHref = trimmedHref
  if (normalizedHref.startsWith('//')) {
    normalizedHref = `https:${normalizedHref}`
  } else if (/^www\./i.test(normalizedHref)) {
    normalizedHref = `https://${normalizedHref}`
  } else if (/^[a-z0-9.-]+\.[a-z]{2,}(?::\d+)?(?:[/?#].*)?$/i.test(normalizedHref)) {
    normalizedHref = `https://${normalizedHref}`
  }

  try {
    const url = new URL(normalizedHref)
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return null
    return url.toString()
  } catch {
    return null
  }
}

function openMarkdownLink(href: string) {
  void openExternalUrl(href).catch((error) => {
    console.error('Failed to open markdown link', error)
  })
}

export function ChatMarkdown({ className, content }: ChatMarkdownProps) {
  const openImagePreview = useImagePreview()
  const markdownClassName = ['chat-markdown', className].filter(Boolean).join(' ')
  const normalizedContent = normalizeMarkdownMath(content)

  return (
    <div className={markdownClassName}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkNormalizeCjkAutolinkBoundaries, remarkMath, remarkBreaks]}
        rehypePlugins={[rehypeKatex]}
        components={{
          a: ({ children, href, ...props }) => {
            const normalizedHref = normalizeMarkdownExternalHref(href)

            return (
              <a
                {...props}
                href={normalizedHref ?? href}
                onClick={(event) => {
                  if (!normalizedHref) return
                  event.preventDefault()
                  openMarkdownLink(normalizedHref)
                }}
                rel="noreferrer"
                target="_blank"
              >
                {children}
              </a>
            )
          },
          img: ({ alt, src, ...props }) => {
            const imageSrc = typeof src === 'string' ? src : ''

            return (
              <img
                {...props}
                alt={alt ?? ''}
                onClick={() =>
                  openImagePreview({
                    alt: alt ?? '',
                    fileName: alt ?? undefined,
                    src: imageSrc
                  })
                }
                role={imageSrc ? 'button' : undefined}
                src={src}
                tabIndex={imageSrc ? 0 : undefined}
                onKeyDown={(event) => {
                  if (!imageSrc) return
                  if (event.key !== 'Enter' && event.key !== ' ') return
                  event.preventDefault()
                  openImagePreview({
                    alt: alt ?? '',
                    fileName: alt ?? undefined,
                    src: imageSrc
                  })
                }}
              />
            )
          }
        }}
      >
        {normalizedContent}
      </ReactMarkdown>
    </div>
  )
}
