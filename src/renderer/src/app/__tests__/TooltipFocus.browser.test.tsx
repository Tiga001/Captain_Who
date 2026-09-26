import { afterEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { Tooltip } from '../../components/overlay/Tooltip'

afterEach(() => vi.useRealTimers())

describe('tooltip focus timing', () => {
  it('keeps immediate keyboard focus details for existing consumers', async () => {
    await render(
      <Tooltip content="Immediate details">
        <button type="button">Existing control</button>
      </Tooltip>
    )
    page.getByRole('button', { name: 'Existing control' }).element().focus()
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('Immediate details')
  })

  it('transfers focus normally while delaying rich node details by one second', async () => {
    await render(
      <>
        <input aria-label="Workflow name" />
        <Tooltip content="Node details" delayMs={1000} delayOnFocus>
          <button type="button">Node</button>
        </Tooltip>
      </>
    )
    page.getByRole('textbox', { name: 'Workflow name' }).element().focus()
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const node = page.getByRole('button', { name: 'Node', exact: true }).element()
    node.focus()
    expect(document.activeElement).toBe(node)
    await vi.advanceTimersByTimeAsync(999)
    expect(page.getByRole('tooltip').query()).toBeNull()
    await vi.advanceTimersByTimeAsync(1)
    await expect.element(page.getByRole('tooltip')).toHaveTextContent('Node details')
  })

  it('cancels delayed focus details when focus leaves before the delay', async () => {
    await render(
      <>
        <Tooltip content="Node details" delayMs={1000} delayOnFocus>
          <button type="button">Node</button>
        </Tooltip>
        <button type="button">Other control</button>
      </>
    )
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    page.getByRole('button', { name: 'Node', exact: true }).element().focus()
    await vi.advanceTimersByTimeAsync(400)
    page.getByRole('button', { name: 'Other control' }).element().focus()
    await vi.advanceTimersByTimeAsync(1000)
    expect(page.getByRole('tooltip').query()).toBeNull()
  })
})
