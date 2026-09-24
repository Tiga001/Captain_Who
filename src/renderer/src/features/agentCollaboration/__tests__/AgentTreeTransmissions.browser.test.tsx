import type { AgentSummary } from '@mycopilot/protocol'
import { useRef, type CSSProperties } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import { AgentTreeView, type AgentTreeLayout } from '../AgentTreeView'
import type { AgentTreeTransmission } from '../agentTreeTransmission'
import '../../../styles/global.css'

beforeEach(() => page.viewport(1100, 760))
afterEach(() => page.viewport(1280, 720))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))
vi.mock('../../auth/AccountAuthContext', () => ({
  useAccountAuth: () => ({ state: { profile: { displayName: 'User', avatarDataUrl: null } } })
}))

const noop = () => undefined
const agents: AgentSummary[] = [
  ['root', null, '主智能体'],
  ['a', 'root', '调研智能体'],
  ['b', 'root', '核验智能体'],
  ['c', 'a', '资料整理'],
  ['d', 'a', '数据分析']
].map(([id, parent, name]) => ({
  agentId: id!,
  parentAgentId: parent,
  taskName: name!,
  taskPath: `/${id}`,
  conversationId: `conversation-${id}`,
  rootAgentId: 'root',
  rootConversationId: 'root-chat',
  projectId: null,
  lifecycle: 'active',
  displayStatus: 'running',
  latestActivityAt: 1,
  model: { displayName: 'Research model', modelConfigId: 'model' }
}))

function transmission(
  id: string,
  sourceAgentId: string | null = 'root',
  targetAgentId: string | null = 'a'
): AgentTreeTransmission {
  return { id, sourceAgentId, targetAgentId, kind: 'message', receivedAt: Date.now() }
}

function Tree({
  transmissions = [],
  layout = 'diagram',
  collapsed = [],
  width = 720,
  theme = classicDarkTheme
}: {
  transmissions?: AgentTreeTransmission[]
  layout?: AgentTreeLayout
  collapsed?: string[]
  width?: number
  theme?: typeof classicDarkTheme | typeof classicLightTheme
}) {
  const scrollRef = useRef<HTMLDivElement>(null)
  return (
    <div
      style={
        {
          ...getFrontendCssVariables(frontendConfig, theme),
          width,
          height: 330,
          display: 'grid',
          background: 'var(--mc-color-surface-main-panel)',
          fontFamily: 'var(--mc-font-family)'
        } as CSSProperties
      }
    >
      <AgentTreeView
        layout={layout}
        agents={agents}
        rootAgentId="root"
        userCollapsed={false}
        onToggleUser={noop}
        collapsedAgentIds={new Set(collapsed)}
        onToggle={noop}
        onOpen={noop}
        registerAvatar={noop}
        scrollRef={scrollRef}
        transmissions={transmissions}
      />
    </div>
  )
}

function finish(element: Element) {
  element.dispatchEvent(
    new AnimationEvent('animationend', {
      bubbles: true,
      animationName: 'agent-tree-transmission-fade'
    })
  )
}

describe('agent tree transmission overlay', () => {
  it('ignores initial history, deduplicates live events, and limits concurrent waves to three', async () => {
    const history = transmission('history')
    const screen = await render(<Tree transmissions={[history]} />)
    expect(screen.container.querySelector('svg.agent-tree__transmissions')).toBeNull()
    const live = [
      history,
      transmission('live-1'),
      transmission('live-2', 'root', 'b'),
      transmission('live-3', 'a', 'c'),
      transmission('live-4', 'b', 'c')
    ]
    await screen.rerender(<Tree transmissions={live} />)
    expect(screen.container.querySelectorAll('.agent-tree__transmission')).toHaveLength(3)
    expect(screen.container.querySelector('[data-transmission-id="live-4"]')).toBeNull()
    finish(screen.container.querySelector('[data-transmission-id="live-1"]')!)
    await expect
      .poll(() => screen.container.querySelector('[data-transmission-id="live-4"]'))
      .not.toBeNull()
    expect(screen.container.querySelectorAll('.agent-tree__transmission')).toHaveLength(3)
    for (const element of screen.container.querySelectorAll('.agent-tree__transmission'))
      finish(element)
    await expect
      .poll(() => screen.container.querySelectorAll('.agent-tree__transmission').length)
      .toBe(0)
    await screen.rerender(<Tree transmissions={[...live]} />)
    expect(screen.container.querySelectorAll('.agent-tree__transmission')).toHaveLength(0)
  })

  it('serializes repeated or opposite-direction traffic on the same connection', async () => {
    const screen = await render(<Tree />)
    await screen.rerender(
      <Tree
        transmissions={[
          transmission('first'),
          transmission('second', 'a', 'root'),
          transmission('third')
        ]}
      />
    )
    expect(screen.container.querySelectorAll('.agent-tree__transmission')).toHaveLength(1)
    expect(screen.container.querySelector('[data-transmission-id="first"]')).not.toBeNull()
    finish(screen.container.querySelector('[data-transmission-id="first"]')!)
    await expect
      .poll(() => screen.container.querySelector('[data-transmission-id="second"]'))
      .not.toBeNull()
    expect(screen.container.querySelectorAll('.agent-tree__transmission')).toHaveLength(1)
  })

  it('cancels active decoration when the keep-alive tree is hidden and never replays it', async () => {
    const screen = await render(<Tree />)
    const events = [transmission('visible')]
    await screen.rerender(<Tree transmissions={events} />)
    expect(screen.container.querySelectorAll('.agent-tree__transmission')).toHaveLength(1)
    const wrapper = screen.container.firstElementChild as HTMLElement
    wrapper.style.display = 'none'
    await expect
      .poll(() => screen.container.querySelectorAll('.agent-tree__transmission').length)
      .toBe(0)
    wrapper.style.display = 'grid'
    await screen.rerender(<Tree transmissions={events} />)
    expect(screen.container.querySelector('.agent-tree__transmission')).toBeNull()
  })

  it('never replays events received in outline or with a collapsed endpoint', async () => {
    const screen = await render(<Tree />)
    const hidden = transmission('hidden', 'a', 'c')
    await screen.rerender(<Tree collapsed={['a']} transmissions={[hidden]} />)
    expect(screen.container.querySelector('svg.agent-tree__transmissions')).toBeNull()
    await screen.rerender(<Tree transmissions={[hidden]} />)
    expect(screen.container.querySelector('svg.agent-tree__transmissions')).toBeNull()
    const outline = transmission('outline')
    await screen.rerender(<Tree layout="outline" transmissions={[hidden, outline]} />)
    await screen.rerender(<Tree transmissions={[hidden, outline]} />)
    expect(screen.container.querySelector('svg.agent-tree__transmissions')).toBeNull()
    const stale = { ...transmission('stale'), receivedAt: Date.now() - 5000 }
    await screen.rerender(<Tree transmissions={[hidden, outline, stale]} />)
    expect(screen.container.querySelector('svg.agent-tree__transmissions')).toBeNull()
  })

  it('uses actual branch direction and an obstacle-avoiding arc without changing layout or scrolling', async () => {
    const screen = await render(<Tree width={280} />)
    const tree = screen.container.querySelector<HTMLDivElement>('.agent-tree')!
    const before = { width: tree.scrollWidth, height: tree.scrollHeight }
    const events = [
      transmission('up', 'a', 'root'),
      transmission('across', 'c', 'b'),
      transmission('human', null, 'root')
    ]
    await screen.rerender(<Tree width={280} transmissions={events} />)
    expect({ width: tree.scrollWidth, height: tree.scrollHeight }).toEqual(before)
    expect(screen.container.querySelector('[data-transmission-id="up"]')).toHaveAttribute(
      'data-route-kind',
      'branch'
    )
    expect(screen.container.querySelector('[data-transmission-id="up"]')).toHaveAttribute(
      'data-source-agent-id',
      'a'
    )
    expect(screen.container.querySelector('[data-transmission-id="across"]')).toHaveAttribute(
      'data-route-kind',
      'arc'
    )
    expect(screen.container.querySelector('[data-transmission-id="human"]')).toHaveAttribute(
      'data-route-kind',
      'branch'
    )
    const checkEndpoints = () => {
      const path = screen.container.querySelector<SVGPathElement>(
        '[data-transmission-id="up"] .agent-tree__transmission-track'
      )!
      const source = screen.container
        .querySelector<HTMLElement>('.agent-tree__node[data-agent-id="a"]')!
        .getBoundingClientRect()
      const target = screen.container
        .querySelector<HTMLElement>('.agent-tree__node[data-agent-id="root"]')!
        .getBoundingClientRect()
      const start = path.getPointAtLength(0).matrixTransform(path.getScreenCTM()!)
      const end = path.getPointAtLength(path.getTotalLength()).matrixTransform(path.getScreenCTM()!)
      return Math.max(
        Math.abs(start.x - (source.left + source.right) / 2),
        Math.abs(start.y - source.top),
        Math.abs(end.x - (target.left + target.right) / 2),
        Math.abs(end.y - target.bottom)
      )
    }
    expect(checkEndpoints()).toBeLessThan(0.1)
    const overlay = tree.querySelector<SVGSVGElement>('.agent-tree__transmissions')!
    expect(getComputedStyle(overlay).pointerEvents).toBe('none')
    const left = overlay.getBoundingClientRect().left
    tree.scrollLeft = 90
    expect(overlay.getBoundingClientRect().left).toBeCloseTo(left - 90)
    expect(tree.scrollLeft).toBe(90)
    await screen.rerender(<Tree width={580} transmissions={events} />)
    await expect
      .poll(() => Number(overlay.getAttribute('width')))
      .toBe(
        Math.max(
          tree.clientWidth,
          tree.querySelector<HTMLElement>('.agent-tree__roots')!.offsetWidth
        )
      )
    const wrapper = screen.container.firstElementChild as HTMLElement
    wrapper.style.zoom = '0.85'
    await expect.poll(checkEndpoints).toBeLessThan(0.2)
  })

  it('removes a naturally completed wave and expires stale queued traffic', async () => {
    const screen = await render(<Tree />)
    const current = transmission('natural')
    const queued = { ...transmission('expiring'), receivedAt: Date.now() - 3900 }
    await screen.rerender(<Tree transmissions={[current, queued]} />)
    expect(screen.container.querySelectorAll('.agent-tree__transmission')).toHaveLength(1)
    await expect
      .poll(() => screen.container.querySelector('.agent-tree__transmissions'), { timeout: 2500 })
      .toBeNull()
    expect(screen.container.querySelector('[data-transmission-id="expiring"]')).toBeNull()
  })

  it('suppresses decoration when reduced motion is requested', async () => {
    const original = window.matchMedia.bind(window)
    const spy = vi.spyOn(window, 'matchMedia').mockImplementation((query) =>
      query === '(prefers-reduced-motion: reduce)'
        ? ({
            ...original(query),
            matches: true,
            addEventListener: noop,
            removeEventListener: noop
          } as MediaQueryList)
        : original(query)
    )
    try {
      const screen = await render(<Tree />)
      await screen.rerender(<Tree transmissions={[transmission('reduced')]} />)
      expect(screen.container.querySelector('.agent-tree__transmissions')).toBeNull()
    } finally {
      spy.mockRestore()
    }
  })

  it.each([
    { name: 'dark', theme: classicDarkTheme, width: 720 },
    { name: 'light', theme: classicLightTheme, width: 720 },
    { name: 'dark-narrow', theme: classicDarkTheme, width: 280 }
  ])(
    'captures a real mid-flight direct branch and curved transfer in the $name diagram',
    async ({ name, theme, width }) => {
      const screen = await render(<Tree theme={theme} width={width} />)
      const tree = screen.container.querySelector<HTMLDivElement>('.agent-tree')!
      tree.scrollLeft = Math.max(0, (tree.scrollWidth - tree.clientWidth) / 2)
      const before = { left: tree.scrollLeft, width: tree.scrollWidth, height: tree.scrollHeight }
      await screen.rerender(
        <Tree
          theme={theme}
          width={width}
          transmissions={[
            transmission('direct'),
            transmission('peer', 'a', 'b'),
            transmission('cross-level', 'root', 'd')
          ]}
        />
      )
      for (const animation of screen.container.getAnimations({ subtree: true })) {
        animation.pause()
        animation.currentTime = 460
      }
      const overlay = screen.container.querySelector<SVGSVGElement>('.agent-tree__transmissions')!
      expect({ left: tree.scrollLeft, width: tree.scrollWidth, height: tree.scrollHeight }).toEqual(
        before
      )
      expect(overlay.querySelectorAll('[data-route-kind="arc"]')).toHaveLength(2)
      const wave = overlay.querySelector<SVGPathElement>('.agent-tree__transmission-wave')!
      expect(Number.parseFloat(getComputedStyle(wave).strokeDashoffset)).toBeLessThan(0)
      await page.screenshot({
        element: screen.container.firstElementChild as HTMLElement,
        path: `__screenshots__/AgentTreeTransmissions.browser.test.tsx/tree-transmissions-${name}-midflight.png`
      })
    }
  )
})
