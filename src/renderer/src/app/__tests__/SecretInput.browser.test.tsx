import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { SecretInput } = await import('../../features/settings/pages/configuration/SecretInput')

function SecretInputHarness() {
  const [value, setValue] = useState('secret-token')
  return <SecretInput ariaLabel="API Token" onChange={setValue} value={value} />
}

describe('SecretInput', () => {
  it('is concealed by default and toggles visibility without changing its value', async () => {
    const screen = await render(<SecretInputHarness />)
    const input = screen.container.querySelector<HTMLInputElement>('input')

    expect(input?.type).toBe('password')
    expect(input?.value).toBe('secret-token')

    await screen.getByRole('button', { name: 'configuration.showSecretValue' }).click()
    expect(input?.type).toBe('text')
    expect(input?.value).toBe('secret-token')

    await screen.getByRole('button', { name: 'configuration.hideSecretValue' }).click()
    expect(input?.type).toBe('password')
    expect(input?.value).toBe('secret-token')
  })
})
