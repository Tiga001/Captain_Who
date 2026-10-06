import type { AgentFileChangeSnapshot } from '@mycopilot/protocol'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { page } from 'vitest/browser'
import type { CSSProperties } from 'react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import { RollingLineCount } from '../components/toolActivities/RollingLineCount'
import {
  FileChangeToolActivityGroup,
  type FileChangeToolActivityGroupItem
} from '../components/toolActivities/FileChangeToolActivity'
import '../ChatConversationPage.agent.css'
import '../../../styles/global.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))
vi.mock('../../agent/agentClient', () => ({
  getAgentFileChangeDiff: vi.fn(),
  getAgentFileChangeHistoryDiff: vi.fn()
}))
vi.mock('../../storage/storageClient', () => ({ revealStoredProjectFile: vi.fn() }))

afterEach(() => vi.restoreAllMocks())

const count = (value: number, sign: '+' | '-' = '+') => (
  <RollingLineCount className="test-count" sign={sign} value={value} />
)
const incoming = (root: ParentNode) => [...root.querySelectorAll('.rolling-line-count__in')]
const outgoing = (root: ParentNode) => [...root.querySelectorAll('.rolling-line-count__out')]
const displayed = (root: ParentNode) =>
  [...root.querySelectorAll('.rolling-line-count__column > span:not(.rolling-line-count__out)')]
    .reverse()
    .map((digit) => digit.textContent)
    .join('')

async function settled(root: ParentNode, value: string) {
  await vi.waitFor(() => {
    expect(incoming(root)).toHaveLength(0)
    expect(outgoing(root)).toHaveLength(0)
    expect(displayed(root)).toBe(value)
  })
}

describe('rolling file-change counts', () => {
  it('keeps compact file rows aligned and colored in light and dark themes as counts roll', async () => {
    await page.viewport(900, 500)
    const view = (value: number) => (
      <div style={{ width: 720 }}>
        {[classicLightTheme, classicDarkTheme].map((theme, index) => (
          <div
            key={index}
            style={
              {
                ...getFrontendCssVariables(frontendConfig, theme),
                padding: 24,
                background: 'var(--mc-color-surface-main-panel)',
                fontFamily: 'var(--mc-font-family)'
              } as CSSProperties
            }
          >
            <div className="agent-activity__details file-change-activity__details">
              <div className="file-change-activity__item-line">
                <span>正在修改</span>
                <span className="file-change-activity__path">
                  04_Final_reports/C001_FRP_Biomolecular_Termination_CSTR/report.md
                </span>
                <span className="file-change-activity__stats">
                  <RollingLineCount
                    className="file-change-activity__additions"
                    sign="+"
                    value={value}
                  />
                  <RollingLineCount
                    className="file-change-activity__deletions"
                    sign="-"
                    value={value}
                  />
                </span>
                <span>›</span>
              </div>
            </div>
          </div>
        ))}
      </div>
    )
    const screen = await render(view(99))
    const rows = [...screen.container.querySelectorAll('.file-change-activity__item-line')]
    const heights = rows.map((row) => row.getBoundingClientRect().height)
    const expectDiffColors = () => {
      rows.forEach((row, index) => {
        for (const [kind, color] of [
          ['additions', 'rgb(22, 163, 74)'],
          ['deletions', index === 0 ? 'rgb(180, 35, 24)' : 'rgb(255, 107, 95)']
        ]) {
          const counter = row.querySelector(`.file-change-activity__${kind}`)!
          // Check the visible sign and every digit layer, not only the colored outer span.
          for (const element of [counter, ...counter.querySelectorAll('span')]) {
            expect(getComputedStyle(element).color).toBe(color)
          }
        }
      })
    }
    expectDiffColors()
    await screen.rerender(view(100))
    const counters = [...screen.container.querySelectorAll('.rolling-line-count')]
    for (const counter of counters) {
      expect(incoming(counter)).toHaveLength(3)
      expect(outgoing(counter)).toHaveLength(2)
    }
    expectDiffColors()
    await Promise.all(counters.map((counter) => settled(counter, '100')))
    expectDiffColors()
    rows.forEach((row, index) => {
      expect(row.getBoundingClientRect().height).toBe(heights[index])
      expect(row.scrollWidth).toBeLessThanOrEqual(row.clientWidth)
    })
    await page.screenshot({
      element: screen.container.firstElementChild as HTMLElement,
      path: '../../../../../../.cache/rolling-line-count/themes.png'
    })
  })

  it('starts static, then rolls changed digits upward without moving the sign', async () => {
    const screen = await render(count(11))
    expect(incoming(screen.container)).toHaveLength(0)
    await screen.rerender(count(33))
    expect(incoming(screen.container).map((digit) => digit.textContent)).toEqual(['3', '3'])
    expect(outgoing(screen.container).map((digit) => digit.textContent)).toEqual(['1', '1'])
    const before = outgoing(screen.container)[0] as HTMLElement
    const after = incoming(screen.container)[0] as HTMLElement
    for (const element of [before, after]) {
      const animation = element.getAnimations()[0]
      animation.pause()
      animation.currentTime = 120
    }
    expect(new DOMMatrix(getComputedStyle(before).transform).m42).toBeLessThan(0)
    expect(new DOMMatrix(getComputedStyle(after).transform).m42).toBeGreaterThan(0)
    expect(
      screen.container.querySelector('.rolling-line-count__visual')?.firstChild?.textContent
    ).toBe('+')
    expect(screen.container.querySelector('.rolling-line-count__text')?.textContent).toBe('+33')
    await settled(screen.container, '33')
    await screen.rerender(count(43))
    expect(incoming(screen.container).map((digit) => digit.textContent)).toEqual(['4'])
    await settled(screen.container, '43')
  })

  it('coalesces rapid updates and settles at the latest total without a playback backlog', async () => {
    const screen = await render(count(11))
    await screen.rerender(count(22))
    for (const value of [33, 44, 55, 66, 77, 88]) await screen.rerender(count(value))
    expect(screen.container.querySelector('.rolling-line-count__text')?.textContent).toBe('+88')
    // The in-flight step finishes; only the newest update follows it.
    await settled(screen.container, '88')
    await screen.rerender(count(88))
    expect(incoming(screen.container)).toHaveLength(0)
  })

  it('aligns place values through carries and shrinking totals, including zero', async () => {
    const screen = await render(count(99, '-'))
    await screen.rerender(count(100, '-'))
    expect(displayed(screen.container)).toBe('100')
    expect(
      (screen.container.querySelector('.rolling-line-count__digits') as HTMLElement).style.width
    ).toBe('3ch')
    await settled(screen.container, '100')
    await screen.rerender(count(9, '-'))
    await settled(screen.container, '9')
    expect(screen.container.querySelectorAll('.rolling-line-count__column')).toHaveLength(1)
    await screen.rerender(count(0, '-'))
    await settled(screen.container, '0')
    expect(screen.container.querySelector('.rolling-line-count__text')?.textContent).toBe('-0')
  })

  it('shows hidden updates immediately when a disclosure opens', async () => {
    const view = (value: number) => (
      <details>
        <summary>Changes</summary>
        {count(value)}
      </details>
    )
    const screen = await render(view(11))
    await screen.rerender(view(33))
    await screen.getByText('Changes').click()
    await settled(screen.container, '33')
    await screen.rerender(view(44))
    expect(incoming(screen.container)).toHaveLength(2)
    await screen.getByText('Changes').click()
    await screen.rerender(view(55))
    await screen.getByText('Changes').click()
    await settled(screen.container, '55')
  })

  it('responds to reduced-motion changes during an active roll', async () => {
    const media = new EventTarget() as MediaQueryList
    Object.defineProperty(media, 'matches', { value: false, writable: true })
    vi.spyOn(window, 'matchMedia').mockReturnValue(media)
    const screen = await render(count(11))
    await screen.rerender(count(33))
    expect(incoming(screen.container)).toHaveLength(2)
    Object.defineProperty(media, 'matches', { value: true })
    media.dispatchEvent(new Event('change'))
    await settled(screen.container, '33')
    await screen.rerender(count(99))
    expect(incoming(screen.container)).toHaveLength(0)
    expect(displayed(screen.container)).toBe('99')
  })

  it('keeps the same staged file row through append calls and final completion', async () => {
    const transaction: AgentFileChangeSnapshot = {
      schemaVersion: 1,
      transactionId: 'draft-one',
      conversationId: 'chat',
      projectId: null,
      filePath: 'report.md',
      operation: 'create',
      updateStrategy: null,
      status: 'drafting',
      baseRevision: null,
      additions: 11,
      deletions: 0,
      lineCount: 11,
      byteCount: 100,
      mutationCount: 1,
      nextMutationIndex: 1,
      statsFinal: false,
      summary: null,
      createdAt: 1,
      updatedAt: 2
    }
    const item = (
      id: string,
      additions: number,
      status: AgentFileChangeSnapshot['status'] = 'drafting'
    ): FileChangeToolActivityGroupItem => ({
      call: {
        id,
        tool: 'apply_patch',
        args: { request: { action: 'append', transactionId: 'draft-one' } },
        approvalStatus: 'not_required',
        reason: ''
      },
      transaction: { ...transaction, additions, status },
      transactionId: 'draft-one'
    })
    const view = (entry: FileChangeToolActivityGroupItem) => (
      <FileChangeToolActivityGroup items={[entry]} />
    )
    const screen = await render(view(item('append-one', 11)))
    await screen.getByText('agent.fileChange.group.running').click()
    const row = screen.container.querySelector('.file-change-activity__item')
    const additions = screen.container.querySelector('.file-change-activity__additions')!
    await screen.rerender(view(item('append-two', 33)))
    expect(screen.container.querySelector('.file-change-activity__item')).toBe(row)
    expect(incoming(additions)).toHaveLength(2)
    await screen.rerender(view(item('commit', 35, 'applied')))
    expect(screen.container.querySelector('.file-change-activity__item')).toBe(row)
    await settled(additions, '35')
    await expect.element(screen.getByText('agent.fileChange.create.row.applied')).toBeVisible()
    expect(
      screen.container.querySelector('.file-change-activity__stats')?.getAttribute('aria-label')
    ).toBe('+35 -0')
  })
})
