// Renderer startup layer: localize the pre-React shell from the existing frontend language catalog.

import {
  DEFAULT_APP_LANGUAGE,
  getLanguageDefinition,
  getTranslation,
  isAppLanguage
} from '../../config/languageRegistry'
import type { AppLanguage, LanguageDirection } from '../../config/languageRegistry'
import { isColorSchemePreference } from '../../config/frontendTheme'
import type { ColorScheme, ColorSchemePreference } from '../../config/frontendTheme'

export interface BootstrapStartupCopy {
  readonly ambient: string
  readonly direction: LanguageDirection
  readonly failedDescription: string
  readonly failedTitle: string
  readonly language: AppLanguage
  readonly loading: string
}

export interface BootstrapStartupAppearance {
  readonly colorScheme: ColorScheme
  readonly preference: ColorSchemePreference
}

function readStoredFrontendConfig(rawStoredConfig: string | null): Record<string, unknown> | null {
  if (!rawStoredConfig) return null
  try {
    const storedConfig: unknown = JSON.parse(rawStoredConfig)
    return storedConfig && typeof storedConfig === 'object' && !Array.isArray(storedConfig)
      ? (storedConfig as Record<string, unknown>)
      : null
  } catch {
    return null
  }
}

export function getBootstrapStartupCopy(rawStoredConfig: string | null): BootstrapStartupCopy {
  const storedConfig = readStoredFrontendConfig(rawStoredConfig)
  const language =
    storedConfig && isAppLanguage(storedConfig.language)
      ? storedConfig.language
      : DEFAULT_APP_LANGUAGE

  return {
    ambient: getTranslation(language, 'startup.ambient.deepThinking'),
    direction: getLanguageDefinition(language).direction,
    failedDescription: getTranslation(language, 'startup.failedDescription'),
    failedTitle: getTranslation(language, 'startup.failedTitle'),
    language,
    loading: getTranslation(language, 'startup.loading')
  }
}

export function getBootstrapStartupAppearance(
  rawStoredConfig: string | null,
  systemColorScheme: ColorScheme
): BootstrapStartupAppearance {
  const storedConfig = readStoredFrontendConfig(rawStoredConfig)
  const preference = isColorSchemePreference(storedConfig?.colorSchemePreference)
    ? storedConfig.colorSchemePreference
    : 'system'

  return {
    colorScheme: preference === 'system' ? systemColorScheme : preference,
    preference
  }
}

export function applyBootstrapStartupAppearance(
  documentRoot: Document,
  appearance: BootstrapStartupAppearance
): void {
  documentRoot.documentElement.dataset.colorScheme = appearance.colorScheme
  documentRoot.documentElement.style.colorScheme = appearance.colorScheme
}

export function applyBootstrapStartupFailure(
  documentRoot: Document,
  rawStoredConfig: string | null
): void {
  const copy = getBootstrapStartupCopy(rawStoredConfig)
  const statusLabel = documentRoot.querySelector<HTMLElement>('[data-bootstrap-startup-label]')
  const ambientText = documentRoot.querySelector<HTMLElement>('[data-bootstrap-startup-ambient]')
  if (ambientText) {
    ambientText.dataset.phase = 'holding'
    ambientText.textContent = copy.failedTitle
  }
  if (statusLabel) statusLabel.textContent = copy.failedDescription
}

export function applyBootstrapStartupLocalization(
  documentRoot: Document,
  rawStoredConfig: string | null
): void {
  const copy = getBootstrapStartupCopy(rawStoredConfig)
  documentRoot.documentElement.lang = copy.language
  documentRoot.documentElement.dir = copy.direction

  const statusLabel = documentRoot.querySelector<HTMLElement>('[data-bootstrap-startup-label]')
  const ambientText = documentRoot.querySelector<HTMLElement>('[data-bootstrap-startup-ambient]')
  if (statusLabel) statusLabel.textContent = copy.loading
  if (ambientText) ambientText.textContent = copy.ambient
}
