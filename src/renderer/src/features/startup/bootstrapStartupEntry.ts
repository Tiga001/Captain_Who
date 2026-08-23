// Renderer startup layer: run before React so the first shell follows the saved app language.

import { FRONTEND_CONFIG_STORAGE_KEY } from '../../config/frontendConfig'
import { applyBootstrapStartupLocalization } from './bootstrapStartupLocalization'

let rawStoredConfig: string | null = null
try {
  rawStoredConfig = window.localStorage.getItem(FRONTEND_CONFIG_STORAGE_KEY)
} catch {
  // Storage access can fail in restricted contexts; the localization adapter has a safe default.
}

applyBootstrapStartupLocalization(document, rawStoredConfig)
