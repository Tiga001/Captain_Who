import { act } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { WorkflowActivityElapsed } from '../../features/workflows/project/WorkflowActivityElapsed'

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const advance = async (milliseconds: number) => {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(milliseconds)
  })
}
beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'] })
  vi.setSystemTime(100_000_000)
})
afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('organization continuous activity label', () => {
  it('updates only its leaf, freezes at the durable end and restarts from the next durable start', async () => {
    const initial = { startedAt: Date.now() - 94_027_000, completedAt: null }
    let parentRenders = 0
    function Parent() {
      parentRenders++
      return <WorkflowActivityElapsed activity={initial} language="zh-CN" />
    }
    const view = await render(<Parent />)
    await expect.element(view.getByText('已连续运行 1d 2h 7m 7s')).toBeVisible()
    const initialRenders = parentRenders
    await advance(2_000)
    await expect.element(view.getByText('已连续运行 1d 2h 7m 9s')).toBeVisible()
    expect(parentRenders).toBe(initialRenders)

    const ended = { ...initial, completedAt: Date.now() - 1_000 }
    await view.rerender(<WorkflowActivityElapsed activity={ended} language="zh-CN" />)
    await expect.element(view.getByText('已连续运行 1d 2h 7m 8s')).toBeVisible()
    expect(vi.getTimerCount()).toBe(0)
    await advance(30_000)
    await expect.element(view.getByText('已连续运行 1d 2h 7m 8s')).toBeVisible()

    const next = { startedAt: Date.now() - 3_000, completedAt: null }
    await view.rerender(<WorkflowActivityElapsed activity={next} language="en-US" />)
    await expect.element(view.getByText('Running for 3s')).toBeVisible()
    await advance(1_000)
    await expect.element(view.getByText('Running for 4s')).toBeVisible()
    await view.unmount()
    expect(vi.getTimerCount()).toBe(0)
    const remounted = await render(<WorkflowActivityElapsed activity={next} language="en-US" />)
    await expect.element(remounted.getByText('Running for 4s')).toBeVisible()
  })

  it('does not create an interval for an organization without activity', async () => {
    const view = await render(<WorkflowActivityElapsed activity={undefined} language="zh-CN" />)
    expect(view.container.textContent).toBe('')
    expect(vi.getTimerCount()).toBe(0)
    await view.rerender(<WorkflowActivityElapsed activity={null} language="en-US" />)
    expect(view.container.textContent).toBe('')
    expect(vi.getTimerCount()).toBe(0)
  })

  it('pauses hidden-page ticks and restores the elapsed wall time immediately on visibility', async () => {
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible')
    const activity = { startedAt: Date.now() - 2_000, completedAt: null }
    const view = await render(<WorkflowActivityElapsed activity={activity} language="zh-CN" />)
    await expect.element(view.getByText('已连续运行 2s')).toBeVisible()
    await act(async () => {
      visibility.mockReturnValue('hidden')
      document.dispatchEvent(new Event('visibilitychange'))
    })
    expect(vi.getTimerCount()).toBe(0)
    await advance(60_000)
    await act(async () => {
      visibility.mockReturnValue('visible')
      document.dispatchEvent(new Event('visibilitychange'))
    })
    await expect.element(view.getByText('已连续运行 1m 2s')).toBeVisible()
  })
})
