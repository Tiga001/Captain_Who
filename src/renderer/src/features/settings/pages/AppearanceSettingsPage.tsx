import { renderSettingsNodes, settingLabel, settingDescription } from '../settingsDefinition'
import { appearanceSettingsNodes, COLOR_SCHEME_OPTIONS } from './AppearanceSettingsPage.definition'
import type { CSSProperties } from 'react'
import { useFrontendConfig } from '../../../config/FrontendConfigProvider'
import { getFrontendTheme } from '../../../config/frontendTheme'
import {
  MAX_UI_CONTRAST,
  MIN_UI_CONTRAST,
  normalizeUiContrast
} from '../../../config/uiContrast'
import type {
  ColorScheme,
  ColorSchemePreference,
  FrontendTheme,
  ThemeIdsByColorScheme
} from '../../../config/frontendTheme'
import type { UiPreferencesSnapshot } from '../../storage/storageClient'
import {
  MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
  MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY,
  normalizeTranslucentSidebarTransparency
} from '../../storage/storageClient'
import { AppearanceThemeSelect } from './AppearanceThemeSelect'
import { AppearanceDiffPreview } from './AppearanceDiffPreview'
import './AppearanceSettingsPage.css'

type ThemePreviewStyle = CSSProperties & {
  '--preview-divider': string
  '--preview-line': string
  '--preview-panel': string
  '--preview-shell': string
  '--preview-window': string
}

function getThemePreviewPalette(theme: FrontendTheme) {
  return {
    divider: theme.colors.border.subtle,
    line: theme.colors.text.muted,
    panel: theme.colors.surface.card,
    shell: theme.colors.surface.leftPanel,
    window: theme.colors.surface.mainPanel
  }
}

function splitPreviewColor(lightColor: string, darkColor: string) {
  return `linear-gradient(90deg, ${lightColor} 0 50%, ${darkColor} 50%)`
}

function getThemePreviewStyle(
  preference: ColorSchemePreference,
  themeIdsByColorScheme: ThemeIdsByColorScheme
): ThemePreviewStyle {
  const lightPalette = getThemePreviewPalette(getFrontendTheme(themeIdsByColorScheme.light).tokens)
  const darkPalette = getThemePreviewPalette(getFrontendTheme(themeIdsByColorScheme.dark).tokens)

  if (preference === 'system') {
    return {
      '--preview-divider': splitPreviewColor(lightPalette.divider, darkPalette.divider),
      '--preview-line': splitPreviewColor(lightPalette.line, darkPalette.line),
      '--preview-panel': splitPreviewColor(lightPalette.panel, darkPalette.panel),
      '--preview-shell': splitPreviewColor(lightPalette.shell, darkPalette.shell),
      '--preview-window': splitPreviewColor(lightPalette.window, darkPalette.window)
    }
  }

  const palette = preference === 'light' ? lightPalette : darkPalette
  return {
    '--preview-divider': palette.divider,
    '--preview-line': palette.line,
    '--preview-panel': palette.panel,
    '--preview-shell': palette.shell,
    '--preview-window': palette.window
  }
}

interface AppearanceSettingsPageProps {
  onUiPreferencesChange: (patch: Partial<UiPreferencesSnapshot>) => void
  uiPreferences: UiPreferencesSnapshot
}

export function AppearanceSettingsPage({
  onUiPreferencesChange,
  uiPreferences
}: AppearanceSettingsPageProps) {
  const {
    colorSchemePreference,
    resolvedThemeId,
    setColorSchemePreference,
    setThemeForColorScheme,
    setUiContrast,
    t,
    themeIdsByColorScheme,
    uiContrast
  } = useFrontendConfig()
  const sidebarTransparency = normalizeTranslucentSidebarTransparency(
    uiPreferences.translucentSidebarTransparency
  )

  return (
    <article className="settings-list-page appearance-settings-page">
      <h1>{t('settings.page.appearance')}</h1>

      {renderSettingsNodes(appearanceSettingsNodes, (section) => {
        switch (section.id) {
          case 'appearance.theme':
            return (
              <section
                className="settings-list-section appearance-theme-section"
                aria-labelledby="appearance-theme-heading"
              >
                <h2 id="appearance-theme-heading">{settingLabel(section, t)}</h2>

                <div
                  className="appearance-theme-grid"
                  role="group"
                  aria-label={settingLabel(section, t)}
                >
                  {COLOR_SCHEME_OPTIONS.map((option) => (
                    <button
                      aria-pressed={colorSchemePreference === option.id}
                      className="appearance-theme-option"
                      data-active={colorSchemePreference === option.id || undefined}
                      key={option.id}
                      onClick={() => setColorSchemePreference(option.id)}
                      type="button"
                    >
                      <span
                        aria-hidden="true"
                        className="appearance-theme-preview"
                        style={getThemePreviewStyle(option.id, themeIdsByColorScheme)}
                      >
                        <span className="appearance-theme-preview__window" />
                        <span className="appearance-theme-preview__header">
                          <span />
                          <span />
                        </span>
                        <span className="appearance-theme-preview__panel">
                          <span />
                          <span />
                          <span />
                        </span>
                      </span>
                      <span className="appearance-theme-option__label">{t(option.labelKey)}</span>
                    </button>
                  ))}
                </div>

                <AppearanceDiffPreview
                  label={t('appearance.diffPreview')}
                  themeId={resolvedThemeId}
                />

                <div className="settings-list appearance-theme-variant-list">
                  {renderSettingsNodes(
                    section.children.filter(
                      (node) =>
                        colorSchemePreference === 'system' ||
                        node.id ===
                          (colorSchemePreference === 'light'
                            ? 'appearance.lightTheme'
                            : 'appearance.darkTheme')
                    ),
                    (node) => {
                      const colorScheme: ColorScheme =
                        node.id === 'appearance.lightTheme' ? 'light' : 'dark'
                      const labelId = `appearance-${colorScheme}-theme-heading`
                      return (
                        <div
                          className="settings-list-row appearance-theme-variant-row"
                          key={colorScheme}
                        >
                          <span className="settings-list-row__text">
                            <span className="settings-list-row__title" id={labelId}>
                              {settingLabel(node, t)}
                            </span>
                          </span>

                          <span className="settings-list-row__control">
                            <AppearanceThemeSelect
                              colorScheme={colorScheme}
                              labelId={labelId}
                              onChange={(themeId) => setThemeForColorScheme(colorScheme, themeId)}
                              value={themeIdsByColorScheme[colorScheme]}
                            />
                          </span>
                        </div>
                      )
                    }
                  )}
                </div>
              </section>
            )
          case 'appearance.preferences':
            return (
              <section
                className="settings-list-section"
                aria-labelledby="appearance-preferences-heading"
              >
                <h2 id="appearance-preferences-heading">{settingLabel(section, t)}</h2>

                <div className="settings-list">
                  {renderSettingsNodes(section.children, (node) => {
                    switch (node.id) {
                      case 'appearance.nativeFontSmoothing':
                        return (
                          <div className="settings-list-row">
                            <div className="settings-list-row__text">
                              <h2
                                className="settings-list-row__title"
                                id="native-font-smoothing-heading"
                              >
                                {settingLabel(node, t)}
                              </h2>
                              <p className="settings-list-row__description">
                                {settingDescription(node, t)}
                              </p>
                            </div>

                            <button
                              className="settings-switch appearance-settings-switch"
                              type="button"
                              role="switch"
                              aria-checked={uiPreferences.nativeFontSmoothing}
                              data-state={uiPreferences.nativeFontSmoothing ? 'on' : 'off'}
                              onClick={() =>
                                onUiPreferencesChange({
                                  nativeFontSmoothing: !uiPreferences.nativeFontSmoothing
                                })
                              }
                            >
                              <span className="settings-switch__thumb" />
                            </button>
                          </div>
                        )
                      case 'appearance.translucentSidebar':
                        return (
                          <div className="settings-list-row">
                            <div className="settings-list-row__text">
                              <h2
                                className="settings-list-row__title"
                                id="translucent-sidebar-heading"
                              >
                                {settingLabel(node, t)}
                              </h2>
                              <p className="settings-list-row__description">
                                {settingDescription(node, t)}
                              </p>
                            </div>

                            <button
                              className="settings-switch appearance-settings-switch"
                              type="button"
                              role="switch"
                              aria-checked={uiPreferences.translucentSidebar}
                              data-state={uiPreferences.translucentSidebar ? 'on' : 'off'}
                              onClick={() =>
                                onUiPreferencesChange({
                                  translucentSidebar: !uiPreferences.translucentSidebar
                                })
                              }
                            >
                              <span className="settings-switch__thumb" />
                            </button>
                          </div>
                        )
                      case 'appearance.uiContrast':
                        return (
                          <div className="settings-list-row appearance-contrast-row">
                            <div className="settings-list-row__text">
                              <h2
                                className="settings-list-row__title"
                                id="appearance-ui-contrast-heading"
                              >
                                {settingLabel(node, t)}
                              </h2>
                              <p className="settings-list-row__description">
                                {settingDescription(node, t)}
                              </p>
                            </div>

                            <label className="appearance-contrast-control">
                              <input
                                type="range"
                                min={MIN_UI_CONTRAST}
                                max={MAX_UI_CONTRAST}
                                step={1}
                                value={uiContrast}
                                aria-labelledby="appearance-ui-contrast-heading"
                                aria-valuetext={`${uiContrast}`}
                                onChange={(event) =>
                                  setUiContrast(
                                    normalizeUiContrast(Number(event.currentTarget.value))
                                  )
                                }
                              />
                              <span className="appearance-contrast-control__value" aria-hidden="true">
                                {uiContrast}
                              </span>
                            </label>
                          </div>
                        )
                      case 'appearance.translucentSidebarTransparency':
                        return (
                          <div
                            className="appearance-translucency-drawer"
                            data-open={uiPreferences.translucentSidebar ? 'true' : 'false'}
                            aria-hidden={!uiPreferences.translucentSidebar}
                          >
                            <div className="settings-list-row appearance-translucency-row">
                              <div className="settings-list-row__text">
                                <h2
                                  className="settings-list-row__title"
                                  id="translucent-sidebar-transparency-heading"
                                >
                                  {settingLabel(node, t)}
                                </h2>
                              </div>

                              <label className="appearance-translucency-control">
                                <input
                                  type="range"
                                  min={MIN_TRANSLUCENT_SIDEBAR_TRANSPARENCY}
                                  max={MAX_TRANSLUCENT_SIDEBAR_TRANSPARENCY}
                                  step={1}
                                  value={sidebarTransparency}
                                  aria-labelledby="translucent-sidebar-transparency-heading"
                                  disabled={!uiPreferences.translucentSidebar}
                                  onChange={(event) =>
                                    onUiPreferencesChange({
                                      translucentSidebarTransparency:
                                        normalizeTranslucentSidebarTransparency(
                                          Number(event.currentTarget.value)
                                        )
                                    })
                                  }
                                />
                              </label>
                            </div>
                          </div>
                        )
                      default:
                        return null
                    }
                  })}
                </div>
              </section>
            )
          default:
            return null
        }
      })}
    </article>
  )
}
