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
    timezone: 'Asia/Shanghai'
  })
  return <AutomationScheduleEditor onChange={setSchedule} schedule={schedule} />
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
        screen.getByRole('button', { name: /automation.permission: automation.permissionFull/ })
      )
      .toBeVisible()
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
