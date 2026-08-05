// Browser coverage for chat Markdown URL boundaries and explicit-link compatibility.

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
