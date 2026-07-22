// Renderer startup tests: verify gated interactivity, retries, and attempt isolation.

import { useEffect } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AppStartupStageId } from '../../features/startup/appStartupStages'

const service = vi.hoisted(() => ({
  ping: vi.fn()
}))

vi.mock('../../host/hostClient', () => ({
  hostClient: {
    core: {
      ping: service.ping
    }
  }
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { AppStartupGate } = await import('../../features/startup/AppStartupGate')
const { AppStartupProvider } = await import('../../features/startup/AppStartupProvider')
const { useAppStartupStage } = await import('../../features/startup/AppStartupContext')

type DataStageId = Exclude<AppStartupStageId, 'core'>

const DATA_STAGE_IDS: DataStageId[] = [
  'modelSettings',
  'projects',
  'uiPreferences',
  'composerDrafts',
  'conversationMetas'
]

function ReadyStage({ stageId }: { stageId: DataStageId }) {
  const { attempt, markReady } = useAppStartupStage(stageId)

  useEffect(() => {
    markReady()
  }, [attempt, markReady])

  return null
}

function FlakyStage({ stageId }: { stageId: DataStageId }) {
  const { attempt, markFailed, markReady } = useAppStartupStage(stageId)

  useEffect(() => {
    if (attempt === 0) {
      markFailed(new Error('stage unavailable'))
    } else {
      markReady()
    }
  }, [attempt, markFailed, markReady])

  return null
}

function StartupHarness({ flakyStage }: { flakyStage?: DataStageId }) {
  return (
    <AppStartupProvider>
      <AppStartupGate>
        <div data-testid="workspace">workspace</div>
        {DATA_STAGE_IDS.map((stageId) =>
          stageId === flakyStage ? (
            <FlakyStage key={stageId} stageId={stageId} />
          ) : (
            <ReadyStage key={stageId} stageId={stageId} />
          )
        )}
      </AppStartupGate>
    </AppStartupProvider>
  )
}

beforeEach(() => {
  service.ping.mockReset().mockResolvedValue({ ok: true })
})

describe('AppStartupGate', () => {
  it('keeps the mounted workspace inert until every blocking stage is ready', async () => {
    let resolveCore: ((value: { ok: boolean }) => void) | undefined
    service.ping.mockImplementation(
      () =>
        new Promise<{ ok: boolean }>((resolve) => {
          resolveCore = resolve
        })
    )

    const screen = await render(<StartupHarness />)
    const workspaceHost = () =>
      screen.container.querySelector<HTMLElement>('.app-startup-workspace')

    await expect.element(screen.getByText('startup.loading')).toBeInTheDocument()
    expect(workspaceHost()?.getAttribute('aria-hidden')).toBe('true')

    await expect.poll(() => service.ping.mock.calls.length).toBe(1)
    resolveCore?.({ ok: true })

    await expect.poll(() => workspaceHost()?.getAttribute('aria-hidden')).toBe('false')
    await expect.element(screen.getByText('startup.loading')).not.toBeInTheDocument()
  })

  it('retries all startup stages with a new attempt after a blocking failure', async () => {
    const screen = await render(<StartupHarness flakyStage="projects" />)
    const workspaceHost = () =>
      screen.container.querySelector<HTMLElement>('.app-startup-workspace')

    await expect.element(screen.getByRole('alert')).toBeInTheDocument()
    await expect.element(screen.getByText('startup.failedTitle')).toBeInTheDocument()

    await screen.getByRole('button', { name: 'startup.retry' }).click()

    await expect.poll(() => workspaceHost()?.getAttribute('aria-hidden')).toBe('false')
    expect(service.ping).toHaveBeenCalledTimes(2)
  })
})
