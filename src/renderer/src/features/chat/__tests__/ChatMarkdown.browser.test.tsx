// Browser coverage for chat Markdown URL boundaries, math normalization, and link compatibility.

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ChatMarkdown } from '../components/ChatMarkdown'

const markdownMocks = vi.hoisted(() => ({
  copyTextToClipboard: vi.fn(async () => undefined),
  highlightState: {
    current: { status: 'idle' } as
      | { status: 'idle' }
      | { status: 'loading' }
      | {
          result: {
            cacheKey: string
            language: string
            lines: Array<{
              line: number
              tokens: Array<{
                color?: string
                content: string
                end: number
                start: number
              }>
            }>
            mode: 'highlighted' | 'plain'
          }
          status: 'ready'
        }
  }
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) =>
      ({
        'chat.codeBlock.plainText': 'Plain text',
        'chat.copied': 'Copied',
        'chat.copy': 'Copy',
        'files.wrapLines': 'Wrap lines'
      })[key] ?? key
  })
}))

vi.mock('../../syntaxHighlighting', () => ({
  useSyntaxHighlight: () => markdownMocks.highlightState.current
}))

vi.mock('../components/chatMessageItemUtils', () => ({
  copyTextToClipboard: markdownMocks.copyTextToClipboard
}))

vi.mock('../../../lib/externalLinks', () => ({
  openExternalUrl: vi.fn()
}))

vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => ({ openImagePreview: vi.fn() })
}))

beforeEach(() => {
  markdownMocks.copyTextToClipboard.mockClear()
  markdownMocks.highlightState.current = { status: 'idle' }
})

describe('ChatMarkdown URL boundaries', () => {
  it('stops a GFM bare URL before adjacent CJK prose', async () => {
    const content =
      '打开 https://example.com，然后尝试查找一个肯定不存在的元素 #mcp-definitely-missing。'
    const screen = await render(<ChatMarkdown content={content} />)
    const link = screen.container.querySelector('a')

    expect(link?.textContent).toBe('https://example.com')
    expect(link?.getAttribute('href')).toBe('https://example.com/')
    expect(screen.container.textContent).toBe(content)
    expect(screen.container.querySelectorAll('a')).toHaveLength(1)
  })

  it('recognizes full-width CJK sentence punctuation as a bare-link boundary', async () => {
    const content = '查看 https://example.com/path。然后继续。'
    const screen = await render(<ChatMarkdown content={content} />)
    const link = screen.container.querySelector('a')

    expect(link?.textContent).toBe('https://example.com/path')
    expect(link?.getAttribute('href')).toBe('https://example.com/path')
    expect(screen.container.textContent).toBe(content)
  })

  it('preserves explicit Markdown links containing CJK punctuation', async () => {
    const content = '[报告，最终版](https://example.com/报告，最终版.pdf)'
    const screen = await render(<ChatMarkdown content={content} />)
    const link = screen.container.querySelector('a')

    expect(link?.textContent).toBe('报告，最终版')
    expect(link?.getAttribute('href')).toContain(
      '/%E6%8A%A5%E5%91%8A%EF%BC%8C%E6%9C%80%E7%BB%88%E7%89%88.pdf'
    )
  })

  it('preserves a bare URL whose CJK punctuation belongs to an existing CJK query', async () => {
    const content = 'https://example.com/search?q=你好,世界'
    const screen = await render(<ChatMarkdown content={content} />)
    const link = screen.container.querySelector('a')

    expect(link?.textContent).toBe(content)
    expect(screen.container.querySelectorAll('a')).toHaveLength(1)
  })
})

describe('ChatMarkdown math normalization', () => {
  it('renders multiline dollar math whose delimiters share the first and last formula lines', async () => {
    const content = String.raw`$$f(x)=\sum_{n=0}^{\infty}\frac{f^{(n)}(a)}{n!}(x-a)^n
=f(a)+f'(a)(x-a)+\cdots$$

**截断与余项**：保留有限项后，正文仍应正常显示。

麦克劳林展开和浙大搜索摘要也不能被吞进公式。`
    const screen = await render(<ChatMarkdown content={content} />)

    expect(screen.container.querySelector('.katex-error')).toBeNull()
    expect(screen.container.querySelector('.katex-display')).not.toBeNull()
    await expect.element(screen.getByText('截断与余项')).toBeVisible()
    expect(screen.container.textContent).toContain('麦克劳林展开和浙大搜索摘要也不能被吞进公式。')
  })

  it('preserves standard math blocks and dollar markers inside fenced code', async () => {
    const content = [
      '$$',
      'x^2+y^2=z^2',
      '$$',
      '',
      '```text',
      '$$not math',
      'still code$$',
      '```'
    ].join('\n')
    const screen = await render(<ChatMarkdown content={content} />)

    expect(screen.container.querySelector('.katex-error')).toBeNull()
    expect(screen.container.querySelectorAll('.katex-display')).toHaveLength(1)
    expect(screen.container.querySelector('pre code')?.textContent).toBe(
      '$$not math\nstill code$$\n'
    )
  })

  it('does not treat a shell prompt and $? as math when math is disabled', async () => {
    const content =
      '$ curl -sS -I --max-time 20 https://upload.wikimedia.org/wikipedia/commons/thumb/6/69/Zhejiang_University_logo.svg/512px-Zhejiang_University_logo.svg.png ; echo "EXIT=$?"运行这个命令'
    const screen = await render(<ChatMarkdown enableMath={false} content={content} />)

    expect(screen.container.querySelector('.katex')).toBeNull()
    expect(screen.container.querySelector('.katex-display')).toBeNull()
    expect(screen.container.textContent).toContain('$ curl')
    expect(screen.container.textContent).toContain('Zhejiang_University_logo')
    expect(screen.container.textContent).toContain('EXIT=$?')
    expect(screen.container.textContent).toContain('运行这个命令')
  })
})

describe('ChatMarkdown fenced code blocks', () => {
  it('renders a compact language toolbar with wrap and copy controls', async () => {
    const screen = await render(
      <ChatMarkdown content={['```ts', 'const answer: number = 42', '```'].join('\n')} />
    )
    const block = screen.container.querySelector('.chat-code-block')
    const wrapButton = screen.container.querySelector<HTMLButtonElement>(
      '[aria-label="Wrap lines"]'
    )

    await expect.element(screen.getByText('TypeScript')).toBeVisible()
    expect(block?.classList.contains('is-wrapped')).toBe(false)
    expect(wrapButton?.getAttribute('aria-pressed')).toBe('false')
    expect(screen.container.querySelector('pre code')?.textContent).toBe(
      'const answer: number = 42\n'
    )

    await screen.getByRole('button', { name: 'Wrap lines' }).click()
    expect(block?.classList.contains('is-wrapped')).toBe(true)
    expect(wrapButton?.getAttribute('aria-pressed')).toBe('true')

    await screen.getByRole('button', { name: 'Copy' }).click()
    expect(markdownMocks.copyTextToClipboard).toHaveBeenCalledWith('const answer: number = 42\n')
    await expect.element(screen.getByRole('button', { name: 'Copied' })).toBeVisible()
  })

  it('renders trusted syntax tokens without changing the source text', async () => {
    markdownMocks.highlightState.current = {
      result: {
        cacheKey: 'chat-code:test',
        language: 'typescript',
        lines: [
          {
            line: 0,
            tokens: [
              {
                color: 'var(--git-review-syntax-keyword)',
                content: 'const',
                end: 5,
                start: 0
              },
              { content: ' value = 1', end: 15, start: 5 }
            ]
          },
          { line: 1, tokens: [] }
        ],
        mode: 'highlighted'
      },
      status: 'ready'
    }

    const screen = await render(
      <ChatMarkdown content={['```typescript', 'const value = 1', '```'].join('\n')} />
    )
    const keyword = screen.container.querySelector<HTMLSpanElement>('.chat-code-block code span')

    expect(keyword?.textContent).toBe('const')
    expect(keyword?.style.color).toBe('var(--git-review-syntax-keyword)')
    expect(screen.container.querySelector('pre code')?.textContent).toBe('const value = 1\n')
  })

  it('keeps inline code on the existing inline-code path', async () => {
    const screen = await render(<ChatMarkdown content={'Use `const value = 1` inline.'} />)

    expect(screen.container.querySelector('.chat-code-block')).toBeNull()
    expect(screen.container.querySelector('code')?.textContent).toBe('const value = 1')
  })

  it('keeps an unsupported fenced language visible while safely using plain text', async () => {
    const screen = await render(
      <ChatMarkdown content={['```mermaid', 'graph TD; A-->B', '```'].join('\n')} />
    )

    await expect.element(screen.getByText('mermaid')).toBeVisible()
    expect(screen.container.querySelector('pre code')?.textContent).toBe('graph TD; A-->B\n')
  })
})
