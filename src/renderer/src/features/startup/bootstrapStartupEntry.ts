// Renderer startup layer: run before React so the first shell follows the saved app language.

import { FRONTEND_CONFIG_STORAGE_KEY } from '../../config/frontendConfig'
import darkBrandMark from '../../../../../resources/brand-mark-dark.png'
import lightBrandMark from '../../../../../resources/brand-mark-light.png'
import type { ColorScheme } from '../../config/frontendTheme'
import {
  applyBootstrapStartupAppearance,
  applyBootstrapStartupFailure,
  applyBootstrapStartupLocalization,
  getBootstrapStartupAppearance,
  getBootstrapStartupCopy
} from './bootstrapStartupLocalization'
import {
  startBootstrapStartupAmbientText,
  stopBootstrapStartupAmbientText
} from './bootstrapStartupAmbientText'

let rawStoredConfig: string | null = null
try {
  rawStoredConfig = window.localStorage.getItem(FRONTEND_CONFIG_STORAGE_KEY)
} catch {
  // Storage access can fail in restricted contexts; the localization adapter has a safe default.
}

function getSystemColorScheme(): ColorScheme {
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

applyBootstrapStartupLocalization(document, rawStoredConfig)
const appearance = getBootstrapStartupAppearance(rawStoredConfig, getSystemColorScheme())
applyBootstrapStartupAppearance(document, appearance)
const darkBrandSource = document.querySelector<HTMLSourceElement>(
  '[data-bootstrap-startup-icon-dark]'
)
const brandImage = document.querySelector<HTMLImageElement>('[data-bootstrap-startup-icon]')
if (appearance.preference === 'system') {
  if (darkBrandSource) {
    darkBrandSource.media = '(prefers-color-scheme: dark)'
    darkBrandSource.srcset = darkBrandMark
  }
  if (brandImage) brandImage.src = lightBrandMark
} else {
  if (darkBrandSource) {
    darkBrandSource.removeAttribute('srcset')
    darkBrandSource.media = 'not all'
  }
  if (brandImage)
    brandImage.src = appearance.colorScheme === 'dark' ? darkBrandMark : lightBrandMark
}
startBootstrapStartupAmbientText(document, getBootstrapStartupCopy(rawStoredConfig).language)

export function completeBootstrapStartup(): void {
  stopBootstrapStartupAmbientText()
}

export function failBootstrapStartup(): void {
  stopBootstrapStartupAmbientText()
  applyBootstrapStartupFailure(document, rawStoredConfig)
}
