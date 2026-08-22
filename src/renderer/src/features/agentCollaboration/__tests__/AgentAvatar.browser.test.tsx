import { expect, it } from 'vitest'
import { render } from 'vitest-browser-react'
import { AgentAvatar } from '../AgentAvatar'

it('renders a decorative, local and stable avatar without exposing a source override', async () => {
  const screen = await render(
    <div>
      <AgentAvatar agentId="agent-stable" />
      <AgentAvatar agentId="agent-stable" className="second-entry-point" />
    </div>
  )
  const avatars = screen.container.querySelectorAll<HTMLElement>('.agent-avatar')
  const images = screen.container.querySelectorAll<HTMLImageElement>('.agent-avatar img')

  expect(avatars).toHaveLength(2)
  expect(images).toHaveLength(2)
  expect(avatars[0]?.getAttribute('aria-hidden')).toBe('true')
  expect(images[0]?.alt).toBe('')
  expect(images[0]?.draggable).toBe(false)
  expect(avatars[0]?.dataset.agentAvatarIndex).toBe(avatars[1]?.dataset.agentAvatarIndex)
  expect(images[0]?.src).toBe(images[1]?.src)

  const resolved = new URL(images[0]!.src, window.location.href)
  if (resolved.protocol === 'http:' || resolved.protocol === 'https:') {
    expect(resolved.origin).toBe(window.location.origin)
  } else {
    expect(['blob:', 'data:', 'file:']).toContain(resolved.protocol)
  }
})
