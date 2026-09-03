import { useState } from 'react'
import type { CredentialMutation, CredentialStatus } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { CredentialInput } =
  await import('../../features/settings/pages/configuration/CredentialInput')

function CredentialHarness({
  applyCommitResult = true,
  onCommit = vi.fn(),
  status = 'configured'
}: {
  applyCommitResult?: boolean
  onCommit?: (mutation: CredentialMutation) => void | Promise<void>
  status?: CredentialStatus
}) {
  const [mutation, setMutation] = useState<CredentialMutation>({ type: 'keep' })
  const [currentStatus, setCurrentStatus] = useState(status)
  return (
    <div>
      <CredentialInput
        ariaLabel="API Key"
        mutation={mutation}
        onCommit={async (nextMutation) => {
          await onCommit(nextMutation)
          if (applyCommitResult) {
            setCurrentStatus(nextMutation.type === 'clear' ? 'missing' : 'configured')
            setMutation({ type: 'keep' })
          }
        }}
        onMutationChange={setMutation}
        placeholder="Enter API key"
        status={currentStatus}
      />
      <output data-testid="mutation">{mutation.type}</output>
    </div>
  )
}

describe('CredentialInput', () => {
  it('renders a blank write-only input when the credential is missing', async () => {
    const screen = await render(<CredentialHarness status="missing" />)
    await expect.element(screen.getByLabelText('API Key')).toHaveValue('')
    expect(screen.container.textContent).not.toContain('configuration.credential.configured')
  })

  it('renders configured and in-place replacement visuals without the old secret', async () => {
    const screen = await render(<CredentialHarness />)
    expect(screen.container.querySelector('input')).toBeNull()
    expect(screen.container.textContent).toContain('••••••••')
    expect(screen.container.textContent).toContain('configuration.credential.configured')

    await screen.getByRole('button', { name: 'configuration.credential.replace' }).click()
    await expect.element(screen.getByLabelText('API Key')).toHaveValue('')
    expect(screen.container.textContent).not.toContain('configuration.credential.configured')
    expect(screen.container.textContent).not.toContain('existing-secret')
  })

  it('cancels replacement with Escape or the inline close control and restores focus', async () => {
    const screen = await render(<CredentialHarness />)
    const edit = screen.getByRole('button', { name: 'configuration.credential.replace' })
    await edit.click()
    const input = screen.getByLabelText('API Key')
    await expect.element(input).toHaveFocus()
    await input.fill('draft-secret')
    await userEvent.keyboard('{Escape}')
    await expect
      .element(screen.getByRole('button', { name: 'configuration.credential.replace' }))
      .toHaveFocus()
    expect(screen.container.textContent).not.toContain('draft-secret')

    await screen.getByRole('button', { name: 'configuration.credential.replace' }).click()
    await screen.getByLabelText('API Key').fill('another-secret')
    await screen.getByRole('button', { name: 'configuration.credential.cancelReplace' }).click()
    await expect
      .element(screen.getByRole('button', { name: 'configuration.credential.replace' }))
      .toHaveFocus()
    expect(screen.container.textContent).not.toContain('another-secret')
  })

  it('uses the standard confirmation dialog for clear and restores focus on cancel', async () => {
    const onCommit = vi.fn()
    const screen = await render(<CredentialHarness onCommit={onCommit} />)
    const clear = screen.getByRole('button', { name: 'configuration.credential.clear' })
    await clear.click()
    await expect.element(screen.getByRole('alertdialog')).toBeVisible()
    await userEvent.keyboard('{Escape}')
    await expect.element(clear).toHaveFocus()
    expect(onCommit).not.toHaveBeenCalled()

    await clear.click()
    await screen.getByRole('button', { name: 'configuration.credential.clearConfirm' }).click()
    await expect.element(screen.getByTestId('mutation')).toHaveTextContent('keep')
    await expect.element(screen.getByLabelText('API Key')).toHaveFocus()
    expect(onCommit).toHaveBeenCalledOnce()
    expect(onCommit).toHaveBeenCalledWith({ type: 'clear' })
  })

  it('commits one replacement on Enter plus blur and keeps eye toggles from committing', async () => {
    const onCommit = vi.fn()
    const screen = await render(
      <CredentialHarness applyCommitResult={false} onCommit={onCommit} status="missing" />
    )
    const input = screen.getByLabelText('API Key')
    await input.fill('new-secret')

    await screen.getByRole('button', { name: 'configuration.showSecretValue' }).click()
    expect(onCommit).not.toHaveBeenCalled()
    await expect.element(input).toHaveAttribute('type', 'text')

    await input.click()
    await userEvent.keyboard('{Enter}{Tab}')
    expect(onCommit).toHaveBeenCalledOnce()
    expect(onCommit).toHaveBeenCalledWith({ type: 'replace', value: 'new-secret' })
  })

  it('freezes the committed replacement until an automatic save settles', async () => {
    let resolveCommit!: () => void
    const onCommit = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveCommit = resolve
        })
    )
    const screen = await render(<CredentialHarness onCommit={onCommit} status="missing" />)
    const input = screen.getByLabelText('API Key')
    await input.fill('committed-key')
    await userEvent.keyboard('{Enter}')

    await expect.element(input).toBeDisabled()
    expect(onCommit).toHaveBeenCalledOnce()
    await expect.element(input).toHaveValue('committed-key')

    resolveCommit()
    await expect
      .element(screen.getByRole('button', { name: 'configuration.credential.replace' }))
      .toBeVisible()
  })

  it('does not accept or render an existing secret or credential reference', async () => {
    const screen = await render(<CredentialHarness />)
    expect(screen.container.innerHTML).not.toMatch(/apiToken|credentialRef|credential_ref/)
    expect(screen.container.querySelector('input')).toBeNull()
  })

  it('allows an unavailable credential to be replaced and cancelled without losing recovery', async () => {
    const screen = await render(<CredentialHarness status="unavailable" />)
    const input = screen.getByLabelText('API Key')
    await expect.element(input).toBeEnabled()
    await expect.element(input).toHaveAttribute('autocomplete', 'new-password')
    await input.fill('replacement-key')
    await userEvent.keyboard('{Escape}')
    await expect.element(screen.getByTestId('mutation')).toHaveTextContent('keep')
    await expect.element(screen.getByLabelText('API Key')).toHaveValue('')
  })

  it('retains a replacement after failure and releases the guard after a synchronous throw', async () => {
    const onCommit = vi.fn(() => {
      throw new Error('safe failure')
    })
    const screen = await render(<CredentialHarness onCommit={onCommit} status="missing" />)
    const input = screen.getByLabelText('API Key')
    await input.fill('retry-key')
    await userEvent.keyboard('{Enter}')
    await expect.poll(() => onCommit.mock.calls.length).toBe(1)
    await expect.element(input).toHaveValue('retry-key')
    await expect.element(input).toBeEnabled()
    await input.click()
    await userEvent.keyboard('{Enter}')
    await expect.poll(() => onCommit.mock.calls.length).toBe(2)
    await expect.element(input).toHaveValue('retry-key')
  })

  it('keeps a configured credential when an immediate clear fails', async () => {
    const onCommit = vi.fn(() => {
      throw new Error('clear failed')
    })
    const screen = await render(<CredentialHarness onCommit={onCommit} />)
    await screen.getByRole('button', { name: 'configuration.credential.clear' }).click()
    await screen.getByRole('button', { name: 'configuration.credential.clearConfirm' }).click()
    await expect.poll(() => onCommit.mock.calls.length).toBe(1)
    expect(onCommit).toHaveBeenCalledWith({ type: 'clear' })
    await expect
      .element(screen.getByRole('button', { name: 'configuration.credential.replace' }))
      .toBeVisible()
    await expect.element(screen.getByRole('alertdialog')).toBeVisible()
  })

  it('starts every new replacement in password mode', async () => {
    const screen = await render(<CredentialHarness status="missing" />)
    const input = screen.getByLabelText('API Key')
    await screen.getByRole('button', { name: 'configuration.showSecretValue' }).click()
    await expect.element(input).toHaveAttribute('type', 'text')
    await input.fill('first-key')
    await userEvent.keyboard('{Enter}')
    await expect
      .element(screen.getByRole('button', { name: 'configuration.credential.replace' }))
      .toBeVisible()
    await screen.getByRole('button', { name: 'configuration.credential.replace' }).click()
    await expect.element(screen.getByLabelText('API Key')).toHaveAttribute('type', 'password')
  })
})
