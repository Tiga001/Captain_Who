import { useState } from 'react'
import { userEvent } from 'vitest/browser'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { ModelConfigPickerOption } from '../../features/modelSelection/ModelConfigPicker'

const changeSpy = vi.fn()
const { ModelConfigPicker } = await import('../../features/modelSelection/ModelConfigPicker')

const OPTIONS: ModelConfigPickerOption[] = [
  { id: 'model-one', label: 'Model One' },
  { disabled: true, id: 'model-retired', label: 'Retired Model' },
  { id: 'model-two', label: 'Model Two' },
  { id: 'model-three', label: 'Model Three' }
]

function PickerHarness() {
  const [value, setValue] = useState<string | null>('model-one')
  return (
    <div style={{ margin: 100, width: 320 }}>
      <ModelConfigPicker
        ariaLabel="Select model"
        emptyLabel="No models"
        onChange={(nextValue) => {
          changeSpy(nextValue)
          setValue(nextValue)
        }}
        options={OPTIONS}
        value={value}
        variant="settings"
      />
    </div>
  )
}

describe('ModelConfigPicker keyboard access', () => {
  it('skips disabled models, navigates boundaries, selects, and restores trigger focus', async () => {
    changeSpy.mockReset()
    const screen = await render(<PickerHarness />)
    const trigger = screen.getByRole('button', { name: 'Select model' })
    ;(trigger.element() as HTMLButtonElement).focus()

    await userEvent.keyboard('{ArrowDown}')
    await expect.element(screen.getByRole('option', { name: 'Retired Model' })).toBeDisabled()
    expect(document.activeElement).toBe(screen.getByRole('option', { name: 'Model Two' }).element())

    await userEvent.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(
      screen.getByRole('option', { name: 'Model Three' }).element()
    )
    await userEvent.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(screen.getByRole('option', { name: 'Model One' }).element())

    await userEvent.keyboard('{End}')
    expect(document.activeElement).toBe(
      screen.getByRole('option', { name: 'Model Three' }).element()
    )
    await userEvent.keyboard('{Home}')
    expect(document.activeElement).toBe(screen.getByRole('option', { name: 'Model One' }).element())

    await userEvent.keyboard('{Escape}')
    await expect.poll(() => document.activeElement).toBe(trigger.element())
    expect(screen.container.querySelector('[role="listbox"]')).toBeNull()

    await userEvent.keyboard('{ArrowUp}')
    expect(document.activeElement).toBe(
      screen.getByRole('option', { name: 'Model Three' }).element()
    )
    await userEvent.keyboard('{Enter}')
    await expect.poll(() => changeSpy).toHaveBeenCalledWith('model-three')
    await expect.poll(() => document.activeElement).toBe(trigger.element())
    await expect.element(trigger).toHaveTextContent('Model Three')
  })
})
