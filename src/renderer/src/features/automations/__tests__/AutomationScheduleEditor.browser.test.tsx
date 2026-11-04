import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { AutomationScheduleInput } from '@mycopilot/protocol'
import { AutomationScheduleEditor } from '../components/AutomationScheduleEditor'
import '../ScheduledPage.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ language: 'en-US', t: (key: string) => key })
}))

const common = { anchorAt: 1_800_000_000_000, timezone: 'Asia/Shanghai' }

function ScheduleHarness({ initialSchedule }: { initialSchedule: AutomationScheduleInput }) {
  const [schedule, setSchedule] = useState(initialSchedule)
  return (
    <>
      <AutomationScheduleEditor onChange={setSchedule} schedule={schedule} />
      <output hidden data-testid="schedule-state">
        {JSON.stringify(schedule)}
      </output>
    </>
  )
}

describe('AutomationScheduleEditor state transitions', () => {
  it('drops hidden custom fields while retaining the saved anchor and timezone without mutating its input', async () => {
    const initialSchedule: AutomationScheduleInput = {
      kind: 'custom',
      frequency: 'yearly',
      interval: 3,
      months: [2],
      monthDays: [29],
      timeMinutes: 150,
      ...common
    }
    const original = structuredClone(initialSchedule)
    const screen = await render(<ScheduleHarness initialSchedule={initialSchedule} />)
    const state = screen.getByTestId('schedule-state')

    await screen.getByRole('button', { name: /automation.customFrequency:/ }).click()
    await screen.getByRole('option', { name: 'automation.customHourly' }).click()
    expect(JSON.parse(state.element().textContent!)).toEqual({
      kind: 'custom',
      frequency: 'hourly',
      interval: 3,
      ...common,
      minuteOfHour: 0
    })

    await screen.getByRole('button', { name: /automation.customFrequency:/ }).click()
    await screen.getByRole('option', { name: 'automation.customWeekly' }).click()
    await screen.getByRole('button', { name: /automation.time:/ }).click()
    await screen.getByRole('option', { name: '02:30', exact: true }).click()
    await screen.getByRole('button', { name: /automation.customFrequency:/ }).click()
    await screen.getByRole('option', { name: 'automation.customDaily' }).click()

    expect(JSON.parse(state.element().textContent!)).toEqual({
      kind: 'custom',
      frequency: 'daily',
      interval: 3,
      timeMinutes: 150,
      ...common
    })
    expect(initialSchedule).toEqual(original)

    await screen.getByRole('button', { name: /automation.repeat:/ }).click()
    await screen.getByRole('option', { name: 'automation.repeatDaily' }).click()
    expect(JSON.parse(state.element().textContent!)).toEqual({
      kind: 'daily',
      timeMinutes: 150,
      anchorAt: common.anchorAt,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
    })
    expect(initialSchedule).toEqual(original)
  })

  it.each(['weekly', 'custom'] as const)(
    'keeps %s weekday selections unique and in calendar order through the real picker',
    async (kind) => {
      const initialSchedule: AutomationScheduleInput =
        kind === 'weekly'
          ? { kind, weekdays: ['friday', 'monday'], timeMinutes: 540, ...common }
          : {
              kind,
              frequency: 'weekly',
              interval: 2,
              weekdays: ['friday', 'monday'],
              timeMinutes: 540,
              ...common
            }
      const screen = await render(<ScheduleHarness initialSchedule={initialSchedule} />)
      await screen.getByRole('button', { name: /automation.weekdays:/ }).click()
      await screen.getByRole('option', { name: 'automation.weekdayWednesday' }).click()
      await screen.getByRole('option', { name: 'automation.weekdayFriday' }).click()
      await screen.getByRole('option', { name: 'automation.weekdayFriday' }).click()

      expect(JSON.parse(screen.getByTestId('schedule-state').element().textContent!)).toEqual({
        ...initialSchedule,
        weekdays: ['monday', 'wednesday', 'friday']
      })
      expect(initialSchedule.weekdays).toEqual(['friday', 'monday'])
    }
  )

  it('orders monthly dates through the real picker and preserves the 31st for backend skip semantics', async () => {
    const initialSchedule: AutomationScheduleInput = {
      kind: 'custom',
      frequency: 'monthly',
      interval: 1,
      monthDays: [31, 1],
      timeMinutes: 540,
      ...common
    }
    const screen = await render(<ScheduleHarness initialSchedule={initialSchedule} />)
    await screen.getByRole('button', { name: /automation.monthDays:/ }).click()
    await screen.getByRole('option', { name: '12', exact: true }).click()
    await screen.getByRole('option', { name: '31', exact: true }).click()
    await screen.getByRole('option', { name: '31', exact: true }).click()

    expect(JSON.parse(screen.getByTestId('schedule-state').element().textContent!)).toEqual({
      ...initialSchedule,
      monthDays: [1, 12, 31]
    })
    expect(initialSchedule.monthDays).toEqual([31, 1])
  })

  it('orders yearly months while retaining February 29 unchanged', async () => {
    const initialSchedule: AutomationScheduleInput = {
      kind: 'custom',
      frequency: 'yearly',
      interval: 1,
      months: [12, 2],
      monthDays: [29],
      timeMinutes: 540,
      ...common
    }
    const screen = await render(<ScheduleHarness initialSchedule={initialSchedule} />)
    await screen.getByRole('button', { name: /automation.months:/ }).click()
    await screen.getByRole('option', { name: 'January', exact: true }).click()
    await screen.getByRole('option', { name: 'December', exact: true }).click()
    await screen.getByRole('option', { name: 'December', exact: true }).click()

    expect(JSON.parse(screen.getByTestId('schedule-state').element().textContent!)).toEqual({
      ...initialSchedule,
      months: [1, 2, 12]
    })
    expect(initialSchedule.months).toEqual([12, 2])
  })

  it.each([540, 547])(
    'offers quarter hours without losing saved minute %s',
    async (timeMinutes) => {
      const initialSchedule: AutomationScheduleInput = { kind: 'daily', timeMinutes, ...common }
      const screen = await render(<ScheduleHarness initialSchedule={initialSchedule} />)
      await screen.getByRole('button', { name: /automation.time:/ }).click()

      const options = Array.from(
        screen.getByRole('listbox').element().querySelectorAll('[role="option"]')
      )
      const quarterHours = Array.from({ length: 96 }, (_, index) => index * 15)
      const expectedValues = [...new Set([...quarterHours, timeMinutes])].sort(
        (left, right) => left - right
      )
      expect(options.map((option) => Number(option.getAttribute('data-value')))).toEqual(
        expectedValues
      )
      expect(options[0].textContent).toBe('00:00')
      expect(options.at(-1)?.textContent).toBe('23:45')
      const currentTime = timeMinutes === 547 ? '09:07' : '09:00'
      await expect
        .element(screen.getByRole('option', { name: currentTime, exact: true }))
        .toHaveAttribute('aria-selected', 'true')
      await screen.getByRole('option', { name: currentTime, exact: true }).click()
      expect(JSON.parse(screen.getByTestId('schedule-state').element().textContent!)).toEqual(
        initialSchedule
      )
    }
  )
})
