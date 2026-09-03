import { useState } from 'react'
import type { CredentialMutation, CredentialStatus } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { SearchMode } from '../../config/modelConfig'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { WebSearchSettings } =
  await import('../../features/settings/pages/configuration/WebSearchSettings')

function Harness({
  onCredentialCommit,
  onModeChange
}: {
  onCredentialCommit: (mutation: CredentialMutation) => void | Promise<void>
  onModeChange: (mode: SearchMode) => void
}) {
  const [status, setStatus] = useState<CredentialStatus>('missing')
  const [mode, setMode] = useState<SearchMode>('disabled')
  return (
    <WebSearchSettings
      onSearchModeChange={(nextMode) => {
        setMode(nextMode)
        onModeChange(nextMode)
      }}
      onTavilyApiKeyCommit={async (mutation) => {
        await onCredentialCommit(mutation)
        setStatus(mutation.type === 'clear' ? 'missing' : 'configured')
      }}
      searchMode={mode}
      tavilyApiKeyStatus={status}
    />
  )
}

describe('WebSearchSettings credential commit', () => {
  it('serializes blur commit with the same click that enables search', async () => {
    const commit = vi.fn()
    const modeChange = vi.fn()
    const screen = await render(<Harness onCredentialCommit={commit} onModeChange={modeChange} />)
    const input = screen.getByLabelText('configuration.tavilyApiKey')
    await input.fill('tavily-key')
    await screen.getByRole('switch', { name: 'configuration.webSearchDisabled' }).click()

    await expect.poll(() => commit.mock.calls.length).toBe(1)
    expect(commit).toHaveBeenCalledWith({ type: 'replace', value: 'tavily-key' })
    expect(modeChange).toHaveBeenCalledOnce()
    expect(modeChange).toHaveBeenCalledWith('auto')
    await expect.element(screen.getByRole('switch')).toHaveAttribute('aria-checked', 'true')
    await expect.element(screen.getByRole('alertdialog')).not.toBeInTheDocument()
  })
})
