import { Check, Palette } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { AnchoredPopover } from '../../../components/overlay/AnchoredPopover'
import { Tooltip, dismissActiveTooltip } from '../../../components/overlay/Tooltip'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { projectWorkflowText, WORKFLOW_COLORS } from './projectWorkflowText'
import './workflowColorPicker.css'

export function WorkflowColorPicker({
  color,
  disabled,
  unavailableColors = [],
  onChange
}: {
  color: string
  disabled: boolean
  unavailableColors?: readonly string[]
  onChange: (color: string) => void
}) {
  const { language } = useFrontendConfig()
  const t = projectWorkflowText(language)
  const [open, setOpen] = useState(false)
  const anchorRef = useRef<HTMLButtonElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const menuId = useId()
  const visible = open && !disabled
  const unavailable = new Set(unavailableColors.map((value) => value.trim().toLowerCase()))
  const normalizedColor = color.trim().toLowerCase()
  const availabilityKey = WORKFLOW_COLORS.map((option) =>
    unavailable.has(option.toLowerCase()) ? '0' : '1'
  ).join('')

  useEffect(() => {
    if (!visible) return
    const outside = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return
      if (!anchorRef.current?.contains(event.target) && !popoverRef.current?.contains(event.target))
        setOpen(false)
    }
    const frame = requestAnimationFrame(() => {
      const popover = popoverRef.current
      const target =
        popover?.querySelector<HTMLButtonElement>('[aria-pressed="true"]:not(:disabled)') ??
        popover?.querySelector<HTMLButtonElement>('button:not(:disabled)') ??
        popover?.querySelector<HTMLDivElement>('[role="dialog"]')
      target?.focus()
    })
    document.addEventListener('pointerdown', outside, true)
    return () => {
      cancelAnimationFrame(frame)
      document.removeEventListener('pointerdown', outside, true)
    }
  }, [visible, normalizedColor, availabilityKey])

  return (
    <>
      <Tooltip content={t('工作流颜色', 'Workflow color')}>
        <button
          ref={anchorRef}
          className="workflow-icon-button workflow-color-picker__trigger"
          type="button"
          aria-label={t('工作流颜色', 'Workflow color')}
          aria-haspopup="dialog"
          aria-expanded={visible}
          aria-controls={visible ? menuId : undefined}
          disabled={disabled}
          style={{ color }}
          onClick={() => {
            dismissActiveTooltip()
            setOpen(!visible)
          }}
        >
          <Palette aria-hidden="true" />
        </button>
      </Tooltip>
      {visible && (
        <AnchoredPopover
          anchorRef={anchorRef}
          popoverRef={popoverRef}
          enabled
          placement="bottom"
          className="workflow-color-picker__popover"
          onClose={() => setOpen(false)}
        >
          <div
            id={menuId}
            role="dialog"
            tabIndex={-1}
            aria-label={t('工作流颜色', 'Workflow color')}
            className="workflow-color-picker__colors"
            onBlur={(event) => {
              if (
                event.relatedTarget instanceof Node &&
                !event.currentTarget.contains(event.relatedTarget)
              )
                setOpen(false)
            }}
          >
            {WORKFLOW_COLORS.map((option, index) => (
              <button
                key={option}
                type="button"
                aria-label={`${t('标记颜色', 'Marker color')} ${index + 1}`}
                aria-pressed={normalizedColor === option.toLowerCase()}
                disabled={unavailable.has(option.toLowerCase())}
                title={
                  unavailable.has(option.toLowerCase())
                    ? t('已被其他工作流使用', 'Used by another workflow')
                    : undefined
                }
                style={{ backgroundColor: option }}
                onClick={() => {
                  if (unavailable.has(option.toLowerCase())) return
                  onChange(option)
                  setOpen(false)
                  anchorRef.current?.focus()
                }}
                onKeyDown={(event) => {
                  const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0
                  if (!step && event.key !== 'Home' && event.key !== 'End') return
                  event.preventDefault()
                  const options = Array.from(
                    popoverRef.current?.querySelectorAll<HTMLButtonElement>(
                      'button:not(:disabled)'
                    ) ?? []
                  )
                  if (options.length === 0) return
                  const currentIndex = options.indexOf(event.currentTarget)
                  const next =
                    event.key === 'Home'
                      ? 0
                      : event.key === 'End'
                        ? options.length - 1
                        : (currentIndex + step + options.length) % options.length
                  options[next]?.focus()
                }}
              >
                {normalizedColor === option.toLowerCase() && <Check aria-hidden="true" />}
              </button>
            ))}
          </div>
        </AnchoredPopover>
      )}
    </>
  )
}
