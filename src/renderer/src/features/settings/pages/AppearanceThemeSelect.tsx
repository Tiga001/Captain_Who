import { useCallback, useId, useRef, useState } from 'react'
import type { CSSProperties, FocusEvent, KeyboardEvent } from 'react'
import { Check, ChevronDown } from 'lucide-react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { getFrontendThemesForColorScheme } from '../../../config/frontendTheme'
import type {
  ColorScheme,
  FrontendTheme,
  FrontendThemeId,
  RegisteredFrontendTheme
} from '../../../config/frontendTheme'
import { useDismissOnOutsidePointer } from '../../../hooks/useDismissOnOutsidePointer'

type ThemeSwatchStyle = CSSProperties & {
  '--theme-swatch-background': string
  '--theme-swatch-border': string
  '--theme-swatch-foreground': string
}

interface AppearanceThemeSelectProps {
  colorScheme: ColorScheme
  labelId: string
  onChange: (themeId: FrontendThemeId) => void
  value: FrontendThemeId
}

function getThemeSwatchStyle(theme: FrontendTheme): ThemeSwatchStyle {
  return {
    '--theme-swatch-background': theme.colors.surface.elevated,
    '--theme-swatch-border': theme.colors.border.default,
    '--theme-swatch-foreground': theme.colors.text.accent
  }
}

function ThemeSwatch({ theme }: { theme: RegisteredFrontendTheme }): React.JSX.Element {
  return (
    <span
      aria-hidden="true"
      className="appearance-theme-swatch"
      style={getThemeSwatchStyle(theme.tokens)}
    >
      Aa
    </span>
  )
}

export function AppearanceThemeSelect({
  colorScheme,
  labelId,
  onChange,
  value
}: AppearanceThemeSelectProps): React.JSX.Element | null {
  const { t } = useFrontendConfig()
  const [isOpen, setOpen] = useState(false)
  const rootRef = useRef<HTMLSpanElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([])
  const listboxId = useId()
  const valueId = useId()
  const options = getFrontendThemesForColorScheme(colorScheme)
  const selectedIndex = Math.max(
    0,
    options.findIndex((theme) => theme.id === value)
  )
  const selectedTheme = options[selectedIndex]

  const closeMenu = useCallback(() => setOpen(false), [])
  useDismissOnOutsidePointer(rootRef, isOpen, closeMenu)

  if (!selectedTheme) return null

  const focusOption = (index: number) => {
    if (options.length === 0) return
    const normalizedIndex = (index + options.length) % options.length
    optionRefs.current[normalizedIndex]?.focus()
  }

  const openMenu = (focusIndex = selectedIndex) => {
    setOpen(true)
    window.requestAnimationFrame(() => focusOption(focusIndex))
  }

  const closeMenuAndRestoreFocus = () => {
    setOpen(false)
    window.requestAnimationFrame(() => triggerRef.current?.focus())
  }

  const selectTheme = (themeId: FrontendThemeId) => {
    onChange(themeId)
    closeMenuAndRestoreFocus()
  }

  const handleBlur = (event: FocusEvent<HTMLSpanElement>) => {
    if (!event.currentTarget.contains(event.relatedTarget)) closeMenu()
  }

  const handleTriggerKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      openMenu(selectedIndex)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      openMenu(selectedIndex)
    } else if (event.key === 'Escape' && isOpen) {
      event.preventDefault()
      closeMenu()
    }
  }

  const handleOptionKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      focusOption(index + 1)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      focusOption(index - 1)
    } else if (event.key === 'Home') {
      event.preventDefault()
      focusOption(0)
    } else if (event.key === 'End') {
      event.preventDefault()
      focusOption(options.length - 1)
    } else if (event.key === 'Escape') {
      event.preventDefault()
      closeMenuAndRestoreFocus()
    }
  }

  return (
    <span
      className="appearance-theme-select"
      data-open={isOpen || undefined}
      onBlur={handleBlur}
      ref={rootRef}
    >
      <button
        aria-controls={isOpen ? listboxId : undefined}
        aria-expanded={isOpen}
        aria-haspopup="listbox"
        aria-labelledby={`${labelId} ${valueId}`}
        className="appearance-theme-select__button"
        onClick={() => (isOpen ? closeMenu() : openMenu())}
        onKeyDown={handleTriggerKeyDown}
        ref={triggerRef}
        type="button"
      >
        <ThemeSwatch theme={selectedTheme} />
        <span className="appearance-theme-select__value" id={valueId}>
          {t(selectedTheme.labelKey)}
        </span>
        <ChevronDown aria-hidden="true" />
      </button>

      {isOpen && (
        <span
          aria-labelledby={labelId}
          className="appearance-theme-select__menu"
          id={listboxId}
          role="listbox"
        >
          {options.map((theme, index) => {
            const isSelected = theme.id === selectedTheme.id
            return (
              <button
                aria-selected={isSelected}
                className="appearance-theme-select__option"
                data-selected={isSelected || undefined}
                key={theme.id}
                onClick={() => selectTheme(theme.id)}
                onKeyDown={(event) => handleOptionKeyDown(event, index)}
                ref={(node) => {
                  optionRefs.current[index] = node
                }}
                role="option"
                tabIndex={isSelected ? 0 : -1}
                type="button"
              >
                <ThemeSwatch theme={theme} />
                <span className="appearance-theme-select__option-label">{t(theme.labelKey)}</span>
                {isSelected && <Check aria-hidden="true" />}
              </button>
            )
          })}
        </span>
      )}
    </span>
  )
}
