import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { copyTextToClipboard } from '../../../components/clipboard'
import { ToastProvider } from '../../../components/toast/ToastProvider'
import type { Translate } from '../../../config/translationFormat'
import { GitReviewCopyButton } from '../GitReviewCopyButton'

const t: Translate = (key) => key

afterEach(() => vi.restoreAllMocks())

describe('Git review copy feedback', () => {
  it('prevents duplicate and parent clicks while copying, then clears success feedback', async () => {
    let finish!: () => void
    const onCopy = vi.fn(() => new Promise<void>((resolve) => (finish = resolve)))
    const parentClick = vi.fn()
    const screen = await render(
      <ToastProvider>
        <div onClick={parentClick}>
          <GitReviewCopyButton label="Copy path" onCopy={onCopy} t={t} />
        </div>
      </ToastProvider>
    )
    const button = screen.getByRole('button', { name: 'Copy path' })

    await button.click()
    await expect.element(button).toBeDisabled()
    ;(button.element() as HTMLButtonElement).click()
    expect(onCopy).toHaveBeenCalledOnce()
    expect(parentClick).not.toHaveBeenCalled()

    finish()
    await expect.element(button).toBeEnabled()
    await expect.element(screen.getByRole('status')).toHaveTextContent('gitReview.copy.success')
    await expect
      .poll(() => screen.container.querySelector('[role="status"]'), { timeout: 3000 })
      .toBeNull()
  })

  it('reports a failed copy and allows another attempt', async () => {
    const onCopy = vi
      .fn()
      .mockRejectedValueOnce(new Error('not available'))
      .mockResolvedValueOnce(undefined)
    const screen = await render(
      <ToastProvider>
        <GitReviewCopyButton label="Copy path" onCopy={onCopy} t={t} />
      </ToastProvider>
    )
    const button = screen.getByRole('button', { name: 'Copy path' })
    await button.click()
    await expect.element(screen.getByRole('status')).toHaveTextContent('gitReview.copy.failed')
    await expect.element(button).toBeEnabled()

    await button.click()
    await expect.element(screen.getByRole('status')).toHaveTextContent('gitReview.copy.success')
    expect(onCopy).toHaveBeenCalledTimes(2)
  })
})

describe('clipboard failure handling', () => {
  it('propagates a rejected Clipboard API write', async () => {
    vi.spyOn(navigator.clipboard, 'writeText').mockRejectedValueOnce(new Error('clipboard denied'))
    await expect(copyTextToClipboard('commit details')).rejects.toThrow('clipboard denied')
  })

  it('rejects an unsuccessful fallback and removes its temporary text field', async () => {
    vi.spyOn(navigator, 'clipboard', 'get').mockReturnValue(undefined as unknown as Clipboard)
    const command = vi.spyOn(document, 'execCommand').mockReturnValue(false)
    const fieldCount = document.querySelectorAll('textarea').length

    await expect(copyTextToClipboard('commit details')).rejects.toThrow('Unable to copy')
    expect(command).toHaveBeenCalledWith('copy')
    expect(document.querySelectorAll('textarea')).toHaveLength(fieldCount)
  })

  it('cleans up the temporary field when fallback execution throws', async () => {
    vi.spyOn(navigator, 'clipboard', 'get').mockReturnValue(undefined as unknown as Clipboard)
    vi.spyOn(document, 'execCommand').mockImplementation(() => {
      throw new Error('copy unavailable')
    })
    const fieldCount = document.querySelectorAll('textarea').length

    await expect(copyTextToClipboard('commit details')).rejects.toThrow('copy unavailable')
    expect(document.querySelectorAll('textarea')).toHaveLength(fieldCount)
  })
})
