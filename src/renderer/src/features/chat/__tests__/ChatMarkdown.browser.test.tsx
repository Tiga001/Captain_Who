// Browser coverage for chat Markdown URL boundaries, math normalization, and link compatibility.

import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ChatMarkdown } from '../components/ChatMarkdown'

vi.mock('../../../lib/externalLinks', () => ({
  openExternalUrl: vi.fn()
}))

vi.mock('../components/ImagePreview', () => ({
  useImagePreview: () => ({ openImagePreview: vi.fn() })
}))

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
})
