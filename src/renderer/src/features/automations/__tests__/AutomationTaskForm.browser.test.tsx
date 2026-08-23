import { useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AutomationScheduleInput } from '@mycopilot/protocol'
import { AutomationTaskForm } from '../components/AutomationTaskForm'
import { AutomationScheduleEditor } from '../components/AutomationScheduleEditor'
import {
  makeAutomationDraft,
  makeAutomationTask,
  testConversation,
  testModel
} from './automationUiFixtures'
import '../ScheduledPage.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

const commonProps = {
  conversations: [testConversation],
  models: [testModel],
  onCancel: vi.fn(),
  onDirtyChange: vi.fn(),
  onOpenPermissionSettings: vi.fn(),
  onSubmittingChange: vi.fn(),
  onSubmit: vi.fn(async () => undefined),
  permissionModeAvailability: { custom: true, full: true },
  projects: [{ id: 'project-1', name: 'Project One', createdAt: 1 }]
}

function CustomScheduleHarness() {
  const [schedule, setSchedule] = useState<AutomationScheduleInput>({
    kind: 'custom',
    frequency: 'daily',
    interval: 1,
    timeMinutes: 9 * 60,
    anchorAt: 1_800_000_000_000,
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  })
  return (
    <>
      <AutomationScheduleEditor onChange={setSchedule} schedule={schedule} />
      <output hidden data-testid="schedule-timezone">
        {schedule.timezone}
      </output>
    </>
  )
}

describe('AutomationTaskForm', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('shows only fields for the selected destination and keeps chat selection bidirectional', async () => {
    const screen = await render(
      <AutomationTaskForm {...commonProps} initialDraft={makeAutomationDraft()} mode="create" />
    )

    await expect.element(screen.getByRole('button', { name: /automation.project/ })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: /automation.model/ })).toBeVisible()
    await screen.getByRole('button', { name: /automation.destination:/ }).click()
    await screen.getByRole('option', { name: 'automation.destinationExistingChat' }).click()

    await expect.element(screen.getByRole('button', { name: /automation.chat:/ })).toBeVisible()
    await expect
      .element(
        screen.getByRole('button', {
          name: /automation.notification: automation.notificationImportantUpdates/
        })
      )
      .toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: /automation.project:/ }))
      .not.toBeInTheDocument()

    await screen.getByRole('button', { name: /automation.chat:/ }).click()
    await screen.getByRole('option', { name: /Automation chat/ }).click()
    await expect.element(screen.getByRole('button', { name: /Automation chat/ })).toBeVisible()

    await screen.getByRole('button', { name: /Automation chat/ }).click()
    await screen.getByRole('option', { name: 'automation.destinationNewChat' }).click()
    await expect.element(screen.getByRole('button', { name: /automation.project:/ })).toBeVisible()
    await expect
      .element(
        screen.getByRole('button', {
          name: /automation.notification: automation.notificationAllRuns/
        })
      )
      .toBeVisible()
  })

  it('truncates long project names in the trigger and menu without stretching the form', async () => {
    const longProjectName = `Long project ${'workspace-'.repeat(40)}`
    const screen = await render(
      <div style={{ width: 480 }}>
        <AutomationTaskForm
          {...commonProps}
          initialDraft={{
            ...makeAutomationDraft(),
            destination: {
              kind: 'new_chat',
              projectBinding: 'project',
              projectId: 'project-long',
              modelId: testModel.id
            }
          }}
          mode="create"
          projects={[{ id: 'project-long', name: longProjectName, createdAt: 1 }]}
        />
      </div>
    )

    const trigger = screen.getByRole('button', {
      name: `automation.project: ${longProjectName}`
    })
    const triggerLabel = trigger.element().querySelector('span')
    expect(triggerLabel).not.toBeNull()
    expect(triggerLabel!.scrollWidth).toBeGreaterThan(triggerLabel!.clientWidth)
    expect(getComputedStyle(triggerLabel!).overflow).toBe('hidden')
    expect(getComputedStyle(triggerLabel!).textOverflow).toBe('ellipsis')
    expect(getComputedStyle(triggerLabel!).whiteSpace).toBe('nowrap')

    await trigger.click()
    const option = screen.getByRole('option', { name: longProjectName })
    const optionLabel = option.element().querySelector('span')
    expect(optionLabel).not.toBeNull()
    expect(optionLabel!.scrollWidth).toBeGreaterThan(optionLabel!.clientWidth)
    expect(getComputedStyle(optionLabel!).overflow).toBe('hidden')
    expect(getComputedStyle(optionLabel!).textOverflow).toBe('ellipsis')
    expect(getComputedStyle(optionLabel!).whiteSpace).toBe('nowrap')
    expect(option.element().scrollWidth).toBeLessThanOrEqual(option.element().clientWidth + 1)
  })

  it('matches the composer model capability badges and offers only enabled models', async () => {
    const imageModel = { ...testModel, displayName: 'Image Model' }
    const textModel = {
      ...testModel,
      id: 'model-text',
      displayName: 'Text Model',
      supportsImage: false
    }
    const disabledModel = {
      ...testModel,
      id: 'model-disabled',
      displayName: 'Disabled Model',
      enabled: false
    }
    const screen = await render(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={makeAutomationDraft()}
        mode="create"
        models={[imageModel, textModel, disabledModel]}
      />
    )

    const imageTrigger = screen.getByRole('button', {
      name: 'automation.model: Image Model'
    })
    const imageCapability = imageTrigger
      .element()
      .querySelector<HTMLElement>('.model-config-picker__selected-capability')
    expect(imageCapability?.textContent).toBe('configuration.image')
    expect(imageCapability?.dataset.supported).toBe('true')

    await imageTrigger.click()
    const imageOption = screen.getByRole('option', { name: /Image Model configuration.image/ })
    const textOption = screen.getByRole('option', { name: /Text Model configuration.text/ })
    await expect.element(imageOption).toBeVisible()
    await expect.element(textOption).toBeVisible()
    await expect
      .element(screen.getByRole('option', { name: /Disabled Model/ }))
      .not.toBeInTheDocument()
    expect(
      imageOption.element().querySelector('.model-config-picker__capability')?.textContent
    ).toBe('configuration.image')
    expect(
      textOption.element().querySelector('.model-config-picker__capability')?.textContent
    ).toBe('configuration.text')

    await textOption.click()
    const textTrigger = screen.getByRole('button', { name: 'automation.model: Text Model' })
    const textCapability = textTrigger
      .element()
      .querySelector<HTMLElement>('.model-config-picker__selected-capability')
    expect(textCapability?.textContent).toBe('configuration.text')
    expect(textCapability?.dataset.supported).toBeUndefined()
  })

  it('does not offer a conversation while its optimistic archive is pending', async () => {
    const screen = await render(
      <AutomationTaskForm
        {...commonProps}
        conversations={[
          testConversation,
          {
            ...testConversation,
            id: 'conversation-pending-archive',
            title: 'Archiving chat',
            pendingArchivedAt: Date.now()
          }
        ]}
        initialDraft={makeAutomationDraft()}
        mode="create"
      />
    )

    await screen.getByRole('button', { name: /automation.destination:/ }).click()
    await screen.getByRole('option', { name: 'automation.destinationExistingChat' }).click()
    await screen.getByRole('button', { name: /automation.chat:/ }).click()

    await expect.element(screen.getByRole('option', { name: /Automation chat/ })).toBeVisible()
    await expect
      .element(screen.getByRole('option', { name: /Archiving chat/ }))
      .not.toBeInTheDocument()
  })

  it('requires the existing full-permission risk confirmation', async () => {
    const screen = await render(
      <AutomationTaskForm {...commonProps} initialDraft={makeAutomationDraft()} mode="create" />
    )

    await screen.getByRole('button', { name: /automation.permission:/ }).click()
    await screen.getByRole('option', { name: 'automation.permissionFull' }).click()
    await expect.element(screen.getByRole('dialog')).toBeVisible()
    await screen.getByRole('button', { name: 'chat.fullPermissionConfirmAction' }).click()
    await expect
      .element(
        screen.getByRole('button', {
          name: /automation.permission: automation.permissionFull/
        })
      )
      .toBeVisible()
  })

  it('reuses chat permission icons and tones without rendering explanatory copy', async () => {
    const screen = await render(
      <AutomationTaskForm {...commonProps} initialDraft={makeAutomationDraft()} mode="create" />
    )

    const trigger = screen.getByRole('button', {
      name: /automation.permission: automation.permissionDefault/
    })
    await expect.element(trigger).toHaveAttribute('data-value', 'default')
    expect(trigger.element().querySelector('svg')).not.toBeNull()
    await trigger.click()
    const full = screen.getByRole('option', { name: 'automation.permissionFull' })
    const custom = screen.getByRole('option', { name: 'automation.permissionCustom' })
    await expect.element(full).toHaveAttribute('data-value', 'full')
    await expect.element(custom).toHaveAttribute('data-value', 'custom')
    expect(full.element().querySelector('svg')).not.toBeNull()
    expect(custom.element().querySelector('svg')).not.toBeNull()
    expect(getComputedStyle(full.element().querySelector('svg')!).color).toBe(
      getComputedStyle(full.element()).color
    )
    expect(getComputedStyle(custom.element().querySelector('svg')!).color).toBe(
      getComputedStyle(custom.element()).color
    )
    expect(screen.container.querySelector('.automation-permission-picker__description')).toBeNull()
  })

  it('renders structured custom hourly and yearly controls without an RRULE editor', async () => {
    const screen = await render(
      <AutomationTaskForm {...commonProps} initialDraft={makeAutomationDraft()} mode="create" />
    )

    await screen.getByRole('button', { name: /automation.repeat:/ }).click()
    await screen.getByRole('option', { name: 'automation.repeatCustom' }).click()
    await screen.getByRole('button', { name: /automation.customFrequency:/ }).click()
    await screen.getByRole('option', { name: 'automation.customHourly' }).click()
    await expect.element(screen.getByLabelText('automation.minuteOfHour')).toBeVisible()

    await screen.getByRole('button', { name: /automation.customFrequency:/ }).click()
    await screen.getByRole('option', { name: 'automation.customYearly' }).click()
    await expect.element(screen.getByRole('button', { name: /automation.months:/ })).toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: /automation.monthDays:/ }))
      .toBeVisible()
    await expect.element(screen.getByText(/RRULE/)).not.toBeInTheDocument()
  })

  it('hides model-derived reasoning while creating but preserves it while editing', async () => {
    const screen = await render(
      <AutomationTaskForm {...commonProps} initialDraft={makeAutomationDraft()} mode="create" />
    )

    await expect.element(screen.getByText('automation.reasoningHigh')).not.toBeInTheDocument()
    await screen.rerender(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={makeAutomationDraft()}
        mode="edit"
        task={makeAutomationTask()}
      />
    )
    await expect.element(screen.getByText('automation.reasoningHigh')).toBeVisible()
  })

  it('switches custom daily, weekly, and monthly fields using structured controls', async () => {
    const screen = await render(<CustomScheduleHarness />)
    await expect.element(screen.getByRole('button', { name: /automation.time:/ })).toBeVisible()

    await screen.getByRole('button', { name: /automation.customFrequency:/ }).click()
    await screen.getByRole('option', { name: 'automation.customWeekly' }).click()
    await expect.element(screen.getByRole('button', { name: /automation.weekdays:/ })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: /automation.time:/ })).toBeVisible()

    await screen.getByRole('button', { name: /automation.customFrequency:/ }).click()
    await screen.getByRole('option', { name: 'automation.customMonthly' }).click()
    await expect
      .element(screen.getByRole('button', { name: /automation.monthDays:/ }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: /automation.weekdays:/ }))
      .not.toBeInTheDocument()

    await screen.getByRole('button', { name: /automation.customFrequency:/ }).click()
    await screen.getByRole('option', { name: 'automation.customDaily' }).click()
    await expect.element(screen.getByRole('button', { name: /automation.time:/ })).toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: /automation.monthDays:/ }))
      .not.toBeInTheDocument()
    await expect
      .element(screen.getByTestId('schedule-timezone'))
      .toHaveTextContent(Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC')
  })

  it('hides timezone controls and always submits the current computer timezone', async () => {
    const onSubmit = vi.fn(async () => undefined)
    const initialDraft = makeAutomationDraft()
    const screen = await render(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={{
          ...initialDraft,
          schedule: { ...initialDraft.schedule, timezone: 'Etc/GMT+12' }
        }}
        mode="create"
        onSubmit={onSubmit}
      />
    )

    await expect
      .element(screen.getByRole('button', { name: /automation.timezone/ }))
      .not.toBeInTheDocument()
    await screen.getByRole('button', { name: 'automation.createTask' }).click()
    await vi.waitFor(() => expect(onSubmit).toHaveBeenCalledOnce())
    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        schedule: expect.objectContaining({
          timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
        })
      })
    )
  })

  it('replaces an existing saved timezone only when the user actually saves an edit', async () => {
    const onSubmit = vi.fn(async () => undefined)
    const initialDraft = makeAutomationDraft()
    const screen = await render(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={{
          ...initialDraft,
          schedule: { ...initialDraft.schedule, timezone: 'Etc/GMT+12' }
        }}
        mode="edit"
        onSubmit={onSubmit}
        task={makeAutomationTask()}
      />
    )

    expect(onSubmit).not.toHaveBeenCalled()
    await screen.getByPlaceholder('automation.taskNamePlaceholder').fill('Edited task')
    await screen.getByRole('button', { name: 'automation.save' }).click()
    await vi.waitFor(() => expect(onSubmit).toHaveBeenCalledOnce())
    expect(onSubmit).toHaveBeenCalledWith(
      expect.objectContaining({
        schedule: expect.objectContaining({
          timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
        })
      })
    )
  })

  it('keeps create disabled until required text is valid', async () => {
    const screen = await render(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={makeAutomationDraft({ title: '', prompt: '' })}
        mode="create"
      />
    )

    const submit = screen.getByRole('button', { name: 'automation.createTask' })
    await expect.element(submit).toBeDisabled()
    await screen.getByPlaceholder('automation.taskNamePlaceholder').fill('Daily brief')
    await screen.getByPlaceholder('automation.promptPlaceholder').fill('Summarize changes')
    await expect.element(submit).toBeEnabled()
  })

  it('preserves a dirty user draft when a newer authoritative revision arrives', async () => {
    const onSubmit = vi.fn(async () => undefined)
    const initialTask = makeAutomationTask()
    const initialDraft = makeAutomationDraft()
    const screen = await render(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={initialDraft}
        mode="edit"
        onSubmit={onSubmit}
        task={initialTask}
      />
    )

    const title = screen.getByPlaceholder('automation.taskNamePlaceholder')
    await title.fill('My unsaved wording')
    await screen.rerender(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={makeAutomationDraft({ title: 'Changed in the background' })}
        mode="edit"
        onSubmit={onSubmit}
        task={makeAutomationTask({ revision: 2, title: 'Changed in the background' })}
      />
    )

    await expect.element(title).toHaveValue('My unsaved wording')
    await screen.getByRole('button', { name: 'automation.save' }).click()
    expect(onSubmit).toHaveBeenCalledWith(expect.objectContaining({ title: 'My unsaved wording' }))
  })

  it('keeps a disabled full permission visible and requires a repair instead of downgrading it', async () => {
    const onOpenPermissionSettings = vi.fn()
    const screen = await render(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={makeAutomationDraft({ permissionMode: 'full' })}
        mode="create"
        onOpenPermissionSettings={onOpenPermissionSettings}
        permissionModeAvailability={{ custom: true, full: false }}
      />
    )

    await expect
      .element(
        screen.getByRole('button', {
          name: /automation.permission: automation.permissionFull/
        })
      )
      .toBeVisible()
    await expect.element(screen.getByText('automation.permissionDisabled')).toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: 'automation.createTask' }))
      .toBeDisabled()
    await screen.getByRole('button', { name: 'automation.openPermissionSettings' }).click()
    expect(onOpenPermissionSettings).toHaveBeenCalledOnce()
  })

  it('renders the task reasoning projection even when the current model profile changed', async () => {
    const screen = await render(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={makeAutomationDraft()}
        mode="edit"
        task={makeAutomationTask({
          destination: {
            kind: 'new_chat',
            projectBinding: 'project',
            projectId: 'project-1',
            modelId: 'model-1',
            reasoning: { source: 'model_config', mode: 'enabled', effort: 'max' }
          }
        })}
      />
    )

    await expect.element(screen.getByText('automation.reasoningXHigh')).toBeVisible()
  })

  it('keeps missing project and model snapshots visible until the user chooses replacements', async () => {
    const missingDestination = {
      kind: 'new_chat' as const,
      projectBinding: 'project' as const,
      projectId: 'project-missing',
      modelId: 'model-missing'
    }
    const screen = await render(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={makeAutomationDraft({ destination: missingDestination })}
        mode="edit"
        task={makeAutomationTask({
          destination: {
            ...missingDestination,
            reasoning: { source: 'model_config', mode: 'enabled', effort: 'high' }
          },
          targetSnapshot: {
            projectName: 'Deleted project',
            conversationTitle: null,
            modelDisplayName: 'Disabled model'
          }
        })}
      />
    )

    await expect.element(screen.getByRole('button', { name: /Deleted project/ })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: /Disabled model/ })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'automation.save' })).toBeDisabled()
  })

  it('preserves unsuccessful-only notifications across destination changes', async () => {
    const screen = await render(
      <AutomationTaskForm
        {...commonProps}
        initialDraft={makeAutomationDraft({ notificationPolicy: 'unsuccessful_only' })}
        mode="create"
      />
    )

    const expectUnsuccessful = async () =>
      expect
        .element(
          screen.getByRole('button', {
            name: /automation.notification: automation.notificationUnsuccessfulOnly/
          })
        )
        .toBeVisible()
    await expectUnsuccessful()
    await screen.getByRole('button', { name: /automation.destination:/ }).click()
    await screen.getByRole('option', { name: 'automation.destinationExistingChat' }).click()
    await expectUnsuccessful()
    await screen.getByRole('button', { name: /automation.destination:/ }).click()
    await screen.getByRole('option', { name: 'automation.destinationNewChat' }).click()
    await expectUnsuccessful()
  })
})
