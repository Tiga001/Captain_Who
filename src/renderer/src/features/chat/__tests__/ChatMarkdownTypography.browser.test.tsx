import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { ChatMarkdown } from '../components/ChatMarkdown'
import '../../../styles/global.css'
import '../ChatConversationPage.messages.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))
vi.mock('../../syntaxHighlighting', () => ({
  useSyntaxHighlight: () => ({ status: 'idle' })
}))
vi.mock('../components/chatMessageItemUtils', () => ({
  copyTextToClipboard: vi.fn(async () => undefined)
}))
vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => vi.fn()
}))
vi.mock('../../../lib/externalLinks', () => ({
  openExternalUrl: vi.fn()
}))

let previousRootStyle: string | null

beforeEach(() => {
  previousRootStyle = document.documentElement.getAttribute('style')
  for (const [key, value] of Object.entries(getFrontendCssVariables())) {
    document.documentElement.style.setProperty(key, value)
  }
})

afterEach(() => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
})

function element(parent: ParentNode, selector: string): HTMLElement {
  const result = parent.querySelector<HTMLElement>(selector)
  if (!result) throw new Error(`Missing ${selector}`)
  return result
}

function textRects(node: HTMLElement) {
  const range = document.createRange()
  const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT)
  const rects: DOMRect[] = []
  for (let text = walker.nextNode(); text; text = walker.nextNode()) {
    range.selectNodeContents(text)
    rects.push(...Array.from(range.getClientRects()))
  }
  return rects.filter((rect) => rect.width > 0 && rect.height > 0)
}

function expectTextInside(node: HTMLElement) {
  const box = node.getBoundingClientRect()
  const rects = textRects(node)
  expect(rects.length).toBeGreaterThan(0)
  for (const rect of rects) {
    expect(rect.left).toBeGreaterThanOrEqual(box.left - 1)
    expect(rect.right).toBeLessThanOrEqual(box.right + 1)
    expect(rect.top).toBeGreaterThanOrEqual(box.top - 1)
    expect(rect.bottom).toBeLessThanOrEqual(box.bottom + 1)
  }
}

function expectStackedChildren(node: HTMLElement) {
  const children = Array.from(node.children).filter(
    (child): child is HTMLElement => child instanceof HTMLElement
  )
  for (let index = 1; index < children.length; index += 1) {
    expect(children[index].getBoundingClientRect().top).toBeGreaterThanOrEqual(
      children[index - 1].getBoundingClientRect().bottom - 1
    )
  }
}

async function renderMarkdown(content: string, width = 320) {
  const screen = await render(
    <div style={{ width }}>
      <ChatMarkdown className="chat-agent-text" content={content} />
    </div>
  )
  await document.fonts.ready
  return { screen, markdown: element(screen.container, '.chat-markdown') }
}

describe('chat Markdown density and readable layout', () => {
  it.each([320, 680])('wraps long CJK and mixed prose without clipping at %ipx', async (width) => {
    const cjk = '中文长段落应保持清晰的行间距离，缩窄聊天区域后仍然完整换行。'.repeat(8)
    const mixed =
      '保留 React 19、TypeScript 与中文混排，路径 `src/renderer/features/chat/very_long_component_name.tsx` 和 https://example.com/documentation/really-long-path?topic=typography 也应能够换行。'
    const { markdown } = await renderMarkdown(`${cjk}\n\n${mixed}\n\n最后一段。`, width)
    const paragraphs = Array.from(markdown.querySelectorAll<HTMLElement>(':scope > p'))

    expect(paragraphs).toHaveLength(3)
    expect(getComputedStyle(markdown).fontSize).toBe('15px')
    expect(Number.parseFloat(getComputedStyle(markdown).lineHeight)).toBeCloseTo(23.25)
    expect(markdown.scrollWidth).toBeLessThanOrEqual(markdown.clientWidth + 1)
    expect(markdown.textContent).toContain(cjk)

    const lines = textRects(paragraphs[0])
    expect(lines.length).toBeGreaterThan(3)
    for (let index = 1; index < lines.length; index += 1) {
      expect(lines[index].top - lines[index - 1].top).toBeCloseTo(23.25, 1)
      expect(lines[index].top).toBeGreaterThanOrEqual(lines[index - 1].bottom)
    }
    for (const paragraph of paragraphs) expectTextInside(paragraph)
    expectStackedChildren(markdown)
    expect(
      paragraphs[1].getBoundingClientRect().top - paragraphs[0].getBoundingClientRect().bottom
    ).toBeCloseTo(9, 1)
    expect(getComputedStyle(paragraphs[2]).marginBottom).toBe('0px')
  })

  it('keeps nested lists and multi-paragraph items distinct without trailing quote gaps', async () => {
    const { markdown } = await renderMarkdown(
      [
        '- 一级列表首段。',
        '',
        '  同一项的第二段，需要保留段落间距。',
        '',
        '  - 子项第一段，继续说明。',
        '    1. 三级列表甲。',
        '    2. 三级列表乙。',
        '  - 子项末段。',
        '',
        '- 第二个一级项目。',
        '',
        '> 引用首段。',
        '>',
        '> 引用尾段。',
        '',
        '引用之后的正文。'
      ].join('\n')
    )
    const firstItem = element(markdown, ':scope > ul > li:first-child')
    const paragraphs = Array.from(firstItem.querySelectorAll<HTMLElement>(':scope > p'))
    const nestedList = element(firstItem, ':scope > ul')
    const quote = element(markdown, 'blockquote')
    const quoteFirst = element(quote, ':scope > :first-child')
    const quoteLast = element(quote, ':scope > :last-child')

    expect(paragraphs).toHaveLength(2)
    expect(markdown.querySelectorAll('ul ul ol')).toHaveLength(1)
    expect(
      paragraphs[1].getBoundingClientRect().top - paragraphs[0].getBoundingClientRect().bottom
    ).toBeCloseTo(6, 1)
    expect(
      nestedList.getBoundingClientRect().top - paragraphs[1].getBoundingClientRect().bottom
    ).toBeCloseTo(3, 1)
    expect(getComputedStyle(nestedList).marginBottom).toBe('0px')
    // Tight list items begin with a text node; their first nested element still needs a gap.
    expect(getComputedStyle(element(nestedList, 'ol')).marginTop).toBe('3px')
    expect(getComputedStyle(quoteFirst).marginTop).toBe('0px')
    expect(getComputedStyle(quoteLast).marginBottom).toBe('0px')
    expect(
      quote.getBoundingClientRect().bottom - quoteLast.getBoundingClientRect().bottom
    ).toBeCloseTo(1.5, 1)
    expect(markdown.scrollWidth).toBeLessThanOrEqual(markdown.clientWidth + 1)
    for (const block of markdown.querySelectorAll<HTMLElement>('ul, ol, blockquote')) {
      expectStackedChildren(block)
    }
    for (const paragraph of markdown.querySelectorAll<HTMLElement>('p')) {
      expectTextInside(paragraph)
    }
    expectStackedChildren(markdown)
  })

  it('preserves code, table and math line heights and exposes long quoted code through scrolling or wrapping', async () => {
    const code = `const description = '${'中英文 mixed content '.repeat(12)}'\nconsole.log(description)\n`
    const { screen, markdown } = await renderMarkdown(
      [
        '> 引用里的代码说明。',
        '>',
        '> ```text',
        ...code
          .trimEnd()
          .split('\n')
          .map((line) => `> ${line}`),
        '> ```',
        '>',
        '> 代码之后的说明。',
        '',
        '| 类型 | 说明 |',
        '| --- | --- |',
        '| 中文与 English | 表格换行后仍保留原来的阅读密度。 |',
        '',
        '$$',
        String.raw`\frac{x^2+y^2}{z}=1`,
        '$$'
      ].join('\n')
    )
    const pre = element(markdown, 'pre')
    const scroller = element(markdown, '.chat-code-block__scroller')
    const quote = element(markdown, 'blockquote')
    const table = element(markdown, 'table')
    const math = element(markdown, '.katex-display > .katex')
    const displayMath = element(markdown, '.katex-display')
    const initialCodeHeight = pre.getBoundingClientRect().height

    expect(getComputedStyle(pre).fontSize).toBe('12px')
    expect(Number.parseFloat(getComputedStyle(pre).lineHeight)).toBeCloseTo(17.4, 1)
    expect(getComputedStyle(pre).marginTop).toBe('0px')
    expect(getComputedStyle(pre).marginBottom).toBe('0px')
    expect(pre.textContent).toBe(code)
    expect(getComputedStyle(scroller).overflowX).toBe('auto')
    expect(scroller.scrollWidth).toBeGreaterThan(scroller.clientWidth)
    scroller.scrollLeft = scroller.scrollWidth
    expect(scroller.scrollLeft).toBeGreaterThan(0)
    expect(markdown.scrollWidth).toBeLessThanOrEqual(markdown.clientWidth + 1)
    expectStackedChildren(quote)

    expect(getComputedStyle(table).fontSize).toBe('14px')
    expect(Number.parseFloat(getComputedStyle(table).lineHeight)).toBeCloseTo(23.8, 1)
    for (const cell of table.querySelectorAll<HTMLElement>('th, td')) expectTextInside(cell)
    expect(markdown.querySelector('.katex-error')).toBeNull()
    expect(
      Number.parseFloat(getComputedStyle(math).lineHeight) /
        Number.parseFloat(getComputedStyle(math).fontSize)
    ).toBeCloseTo(1.2, 1)
    const visibleMath = element(math, '.katex-html')
    expect(visibleMath.getBoundingClientRect().top).toBeGreaterThanOrEqual(
      displayMath.getBoundingClientRect().top
    )
    expect(visibleMath.getBoundingClientRect().bottom).toBeLessThanOrEqual(
      displayMath.getBoundingClientRect().bottom
    )
    const mathLayout = () => {
      const box = displayMath.getBoundingClientRect()
      return {
        height: box.height,
        glyphs: textRects(visibleMath).map((rect) => ({
          left: rect.left - box.left,
          top: rect.top - box.top,
          width: rect.width,
          height: rect.height
        }))
      }
    }
    const compactMathLayout = mathLayout()
    markdown.style.setProperty('--mc-line-height-chat', '1.7')
    expect(mathLayout()).toEqual(compactMathLayout)
    markdown.style.removeProperty('--mc-line-height-chat')

    await screen.getByRole('button', { name: 'files.wrapLines' }).click()
    expect(element(markdown, '.chat-code-block').classList.contains('is-wrapped')).toBe(true)
    expect(scroller.scrollWidth).toBeLessThanOrEqual(scroller.clientWidth + 1)
    expect(pre.getBoundingClientRect().height).toBeGreaterThan(initialCodeHeight)
    expect(pre.textContent).toBe(code)
    expectTextInside(element(pre, 'code'))
    expectStackedChildren(quote)
    expect(markdown.scrollWidth).toBeLessThanOrEqual(markdown.clientWidth + 1)
  })
})
