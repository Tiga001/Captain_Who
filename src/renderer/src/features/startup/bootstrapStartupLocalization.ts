// Renderer startup layer: localize the pre-React shell from the existing frontend language catalog.

import {
  DEFAULT_APP_LANGUAGE,
  getLanguageDefinition,
  getTranslation,
  isAppLanguage
} from '../../config/languageRegistry'
import type { AppLanguage, LanguageDirection } from '../../config/languageRegistry'

export interface BootstrapStartupCopy {
  readonly ambient: string
  readonly direction: LanguageDirection
  readonly failedDescription: string
  readonly failedTitle: string
  readonly language: AppLanguage
  readonly loading: string
}

export function getBootstrapStartupCopy(rawStoredConfig: string | null): BootstrapStartupCopy {
  let language = DEFAULT_APP_LANGUAGE

  if (rawStoredConfig) {
    try {
      const storedConfig: unknown = JSON.parse(rawStoredConfig)
      if (
        typeof storedConfig === 'object' &&
        storedConfig !== null &&
        'language' in storedConfig &&
        isAppLanguage(storedConfig.language)
      ) {
        language = storedConfig.language
      }
    } catch {
      // A malformed preference must not block the static startup shell.
    }
  }

  return {
    ambient: getTranslation(language, 'startup.ambient.deepThinking'),
    direction: getLanguageDefinition(language).direction,
    failedDescription: getTranslation(language, 'startup.failedDescription'),
    failedTitle: getTranslation(language, 'startup.failedTitle'),
    language,
    loading: getTranslation(language, 'startup.loading')
  }
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
