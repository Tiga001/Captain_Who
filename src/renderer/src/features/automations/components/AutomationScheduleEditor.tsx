import type {
  AutomationCustomFrequency,
  AutomationIntervalUnit,
  AutomationScheduleInput,
  AutomationWeekday
} from '@mycopilot/protocol'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { WEEKDAYS, WEEKDAY_KEYS } from '../automationPresentation'
import {
  AutomationMultiSelect,
  AutomationSelect,
  type AutomationOption
} from './AutomationControls'
import { getSystemTimeZone } from '../automationSchedule'

type RepeatKind = AutomationScheduleInput['kind']

interface AutomationScheduleEditorProps {
  disabled?: boolean
  errors?: Readonly<Record<string, string>>
  onChange: (schedule: AutomationScheduleInput) => void
  schedule: AutomationScheduleInput
}

const CUSTOM_FREQUENCIES: AutomationCustomFrequency[] = [
  'hourly',
  'daily',
  'weekly',
  'monthly',
  'yearly'
]

function toTimeValue(minutes: number): string {
  const normalized = Math.max(0, Math.min(1439, minutes))
  return `${String(Math.floor(normalized / 60)).padStart(2, '0')}:${String(normalized % 60).padStart(2, '0')}`
}

function quarterHourOptions(currentMinutes: number): AutomationOption<string>[] {
  const values = Array.from({ length: 96 }, (_, index) => index * 15)
  if (!values.includes(currentMinutes)) values.push(currentMinutes)
  return values
    .sort((left, right) => left - right)
    .map((minutes) => ({
      label: toTimeValue(minutes),
      value: String(minutes)
    }))
}

function createScheduleForKind(
  kind: RepeatKind,
  current: AutomationScheduleInput
): AutomationScheduleInput {
  const timezone = getSystemTimeZone()
  const anchorAt = current.anchorAt
  const timeMinutes =
    'timeMinutes' in current && current.timeMinutes !== undefined
      ? current.timeMinutes
      : new Date().getHours() * 60 + new Date().getMinutes()

  if (kind === 'interval') {
    return { kind, amount: 15, unit: 'minutes', anchorAt, timezone }
  }
  if (kind === 'daily' || kind === 'weekdays') {
    return { kind, timeMinutes, anchorAt, timezone }
  }
  if (kind === 'weekly') {
    return { kind, weekdays: ['monday'], timeMinutes, anchorAt, timezone }
  }
  return {
    kind,
    frequency: 'daily',
    interval: 1,
    timeMinutes,
    anchorAt,
    timezone
  }
}

export function AutomationScheduleEditor({
  disabled = false,
  errors = {},
  onChange,
  schedule
}: AutomationScheduleEditorProps) {
  const { t } = useFrontendConfig()
  const repeatOptions: AutomationOption<RepeatKind>[] = [
    { value: 'interval', label: t('automation.repeatInterval') },
    { value: 'daily', label: t('automation.repeatDaily') },
    { value: 'weekdays', label: t('automation.repeatWeekdays') },
    { value: 'weekly', label: t('automation.repeatWeekly') },
    { value: 'custom', label: t('automation.repeatCustom') }
  ]
  const intervalUnitOptions: AutomationOption<AutomationIntervalUnit>[] = [
    { value: 'minutes', label: t('automation.intervalMinutes') },
    { value: 'hours', label: t('automation.intervalHours') },
    { value: 'days', label: t('automation.intervalDays') }
  ]
  const updateTime = (timeMinutes: number) => {
    if (!('timeMinutes' in schedule)) return
    onChange({ ...schedule, timeMinutes } as AutomationScheduleInput)
  }

  const renderTimeRow = (timeMinutes: number) => (
    <AutomationField label={t('automation.time')} error={errors.timeMinutes}>
      <AutomationSelect
        ariaLabel={t('automation.time')}
        disabled={disabled}
        options={quarterHourOptions(timeMinutes)}
        value={String(timeMinutes)}
        onChange={(value) => updateTime(Number(value))}
      />
    </AutomationField>
  )

  return (
    <section className="automation-form__section" aria-labelledby="automation-frequency-heading">
      <h3 id="automation-frequency-heading">{t('automation.frequency')}</h3>
      <div className="automation-form__card">
        <AutomationField label={t('automation.repeat')} error={errors.kind}>
          <AutomationSelect
            ariaLabel={t('automation.repeat')}
            disabled={disabled}
            options={repeatOptions}
            value={schedule.kind}
            onChange={(kind) => onChange(createScheduleForKind(kind, schedule))}
          />
        </AutomationField>

        {schedule.kind === 'interval' && (
          <AutomationField label={t('automation.intervalEvery')} error={errors.amount}>
            <div className="automation-form__inline-control">
              <input
                aria-label={t('automation.intervalEvery')}
                disabled={disabled}
                min={1}
                step={1}
                type="number"
                value={schedule.amount}
                onChange={(event) =>
                  onChange({ ...schedule, amount: Number(event.currentTarget.value) })
                }
              />
              <AutomationSelect
                ariaLabel={t('automation.repeatInterval')}
                disabled={disabled}
                options={intervalUnitOptions}
                value={schedule.unit}
                onChange={(unit) => onChange({ ...schedule, unit })}
              />
            </div>
          </AutomationField>
        )}

        {(schedule.kind === 'daily' || schedule.kind === 'weekdays') &&
          renderTimeRow(schedule.timeMinutes)}

        {schedule.kind === 'weekly' && (
          <>
            <AutomationField label={t('automation.weekdays')} error={errors.weekdays}>
              <AutomationMultiSelect<AutomationWeekday>
                ariaLabel={t('automation.weekdays')}
                disabled={disabled}
                formatValue={(value) => t(WEEKDAY_KEYS[value])}
                options={WEEKDAYS}
                values={schedule.weekdays}
                onChange={(weekdays) => onChange({ ...schedule, weekdays })}
              />
            </AutomationField>
            {renderTimeRow(schedule.timeMinutes)}
          </>
        )}

        {schedule.kind === 'custom' && (
          <CustomScheduleFields
            disabled={disabled}
            errors={errors}
            onChange={onChange}
            schedule={schedule}
          />
        )}
      </div>
    </section>
  )
}

interface CustomScheduleFieldsProps {
  disabled: boolean
  errors: Readonly<Record<string, string>>
  onChange: (schedule: AutomationScheduleInput) => void
  schedule: Extract<AutomationScheduleInput, { kind: 'custom' }>
}

function CustomScheduleFields({ disabled, errors, onChange, schedule }: CustomScheduleFieldsProps) {
  const { language, t } = useFrontendConfig()
  const timeMinutes =
    'timeMinutes' in schedule && schedule.timeMinutes !== undefined ? schedule.timeMinutes : 9 * 60
  const frequencyOptions: AutomationOption<AutomationCustomFrequency>[] = CUSTOM_FREQUENCIES.map(
    (frequency) => ({
      value: frequency,
      label: t(
        frequency === 'hourly'
          ? 'automation.customHourly'
          : frequency === 'daily'
            ? 'automation.customDaily'
            : frequency === 'weekly'
              ? 'automation.customWeekly'
              : frequency === 'monthly'
                ? 'automation.customMonthly'
                : 'automation.customYearly'
      )
    })
  )

  const setFrequency = (frequency: AutomationCustomFrequency) => {
    const common = {
      interval: schedule.interval,
      anchorAt: schedule.anchorAt,
      timezone: schedule.timezone
    }
    if (frequency === 'hourly') {
      onChange({ kind: 'custom', frequency: 'hourly', ...common, minuteOfHour: 0 })
    } else if (frequency === 'weekly') {
      onChange({
        kind: 'custom',
        frequency: 'weekly',
        ...common,
        weekdays: ['monday'],
        timeMinutes
      })
    } else if (frequency === 'monthly') {
      onChange({
        kind: 'custom',
        frequency: 'monthly',
        ...common,
        monthDays: [1],
        timeMinutes
      })
    } else if (frequency === 'yearly') {
      onChange({
        kind: 'custom',
        frequency: 'yearly',
        ...common,
        months: [1],
        monthDays: [1],
        timeMinutes
      })
    } else {
      onChange({ kind: 'custom', frequency: 'daily', ...common, timeMinutes })
    }
  }

  const setTime = (value: string) => {
    if (!('timeMinutes' in schedule)) return
    onChange({ ...schedule, timeMinutes: Number(value) } as AutomationScheduleInput)
  }

  return (
    <>
      <AutomationField label={t('automation.customFrequency')}>
        <AutomationSelect
          ariaLabel={t('automation.customFrequency')}
          disabled={disabled}
          options={frequencyOptions}
          value={schedule.frequency}
          onChange={setFrequency}
        />
      </AutomationField>
      <AutomationField label={t('automation.everyN')} error={errors.interval}>
        <input
          aria-label={t('automation.everyN')}
          disabled={disabled}
          min={1}
          step={1}
          type="number"
          value={schedule.interval}
          onChange={(event) =>
            onChange({
              ...schedule,
              interval: Number(event.currentTarget.value)
            } as AutomationScheduleInput)
          }
        />
      </AutomationField>
      {schedule.frequency === 'hourly' && (
        <AutomationField label={t('automation.minuteOfHour')} error={errors.minuteOfHour}>
          <input
            aria-label={t('automation.minuteOfHour')}
            disabled={disabled}
            max={59}
            min={0}
            step={1}
            type="number"
            value={schedule.minuteOfHour}
            onChange={(event) =>
              onChange({ ...schedule, minuteOfHour: Number(event.currentTarget.value) })
            }
          />
        </AutomationField>
      )}
      {schedule.frequency === 'weekly' && (
        <AutomationField label={t('automation.weekdays')} error={errors.weekdays}>
          <AutomationMultiSelect<AutomationWeekday>
            ariaLabel={t('automation.weekdays')}
            disabled={disabled}
            formatValue={(value) => t(WEEKDAY_KEYS[value])}
            options={WEEKDAYS}
            values={schedule.weekdays}
            onChange={(weekdays) => onChange({ ...schedule, weekdays })}
          />
        </AutomationField>
      )}
      {(schedule.frequency === 'monthly' || schedule.frequency === 'yearly') && (
        <AutomationField label={t('automation.monthDays')} error={errors.monthDays}>
          <AutomationMultiSelect<number>
            ariaLabel={t('automation.monthDays')}
            disabled={disabled}
            formatValue={String}
            options={Array.from({ length: 31 }, (_, index) => index + 1)}
            values={schedule.monthDays}
            onChange={(monthDays) => onChange({ ...schedule, monthDays })}
          />
        </AutomationField>
      )}
      {schedule.frequency === 'yearly' && (
        <AutomationField label={t('automation.months')} error={errors.months}>
          <AutomationMultiSelect<number>
            ariaLabel={t('automation.months')}
            disabled={disabled}
            formatValue={(month) =>
              new Intl.DateTimeFormat(language, { month: 'long', timeZone: 'UTC' }).format(
                Date.UTC(2024, month - 1, 1)
              )
            }
            options={Array.from({ length: 12 }, (_, index) => index + 1)}
            values={schedule.months}
            onChange={(months) => onChange({ ...schedule, months })}
          />
        </AutomationField>
      )}
      {schedule.frequency !== 'hourly' && (
        <AutomationField label={t('automation.time')} error={errors.timeMinutes}>
          <AutomationSelect
            ariaLabel={t('automation.time')}
            disabled={disabled}
            options={quarterHourOptions(timeMinutes)}
            value={String(timeMinutes)}
            onChange={setTime}
          />
        </AutomationField>
      )}
    </>
  )
}

export function AutomationField({
  children,
  error,
  label
}: {
  children: React.ReactNode
  error?: string
  label: string
}) {
  return (
    <div className="automation-form__field" data-invalid={Boolean(error) || undefined}>
      <div className="automation-form__field-row">
        <span className="automation-form__label">{label}</span>
        <div className="automation-form__control">{children}</div>
      </div>
      {error && (
        <p className="automation-form__field-error" role="alert">
          {error}
        </p>
      )}
    </div>
  )
}
