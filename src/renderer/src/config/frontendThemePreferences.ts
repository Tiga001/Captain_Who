import {
  defaultThemeIdsByColorScheme,
  getDefaultThemeIdForColorScheme,
  getFrontendTheme,
  isColorSchemePreference,
  isThemeIdForColorScheme
} from './frontendTheme'
import type {
  ColorScheme,
  ColorSchemePreference,
  FrontendThemeId,
  FrontendThemeIdForColorScheme,
  RegisteredFrontendTheme,
  ThemeIdsByColorScheme
} from './frontendTheme'

export const FRONTEND_THEME_PREFERENCES_VERSION = 3

const legacyThemeIdAliases: Readonly<Record<string, FrontendThemeId>> = {
  'catppuccin-dark': 'catppuccin-mocha-dark'
}

export interface FrontendThemePreferences {
  colorSchemePreference: ColorSchemePreference
  themeIdsByColorScheme: ThemeIdsByColorScheme
}

export interface ResolvedFrontendTheme {
  colorScheme: ColorScheme
  theme: RegisteredFrontendTheme
  themeId: FrontendThemeId
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

export function normalizeThemeIdForColorScheme<Scheme extends ColorScheme>(
  colorScheme: Scheme,
  value: unknown,
  fallback: unknown = getDefaultThemeIdForColorScheme(colorScheme)
): FrontendThemeIdForColorScheme<Scheme> {
  const normalizedValue = typeof value === 'string' ? (legacyThemeIdAliases[value] ?? value) : value
  const normalizedFallback =
    typeof fallback === 'string' ? (legacyThemeIdAliases[fallback] ?? fallback) : fallback

  if (isThemeIdForColorScheme(normalizedValue, colorScheme)) return normalizedValue
  if (isThemeIdForColorScheme(normalizedFallback, colorScheme)) return normalizedFallback
  return getDefaultThemeIdForColorScheme(colorScheme)
}

export function normalizeThemeIdsByColorScheme(
  value: unknown,
  defaults: ThemeIdsByColorScheme = defaultThemeIdsByColorScheme
): ThemeIdsByColorScheme {
  const storedThemeIds = isRecord(value) ? value : {}
  return {
    light: normalizeThemeIdForColorScheme('light', storedThemeIds.light, defaults.light),
    dark: normalizeThemeIdForColorScheme('dark', storedThemeIds.dark, defaults.dark)
  }
}

export function normalizeFrontendThemePreferences(
  value: unknown,
  defaults: FrontendThemePreferences = {
    colorSchemePreference: 'system',
    themeIdsByColorScheme: defaultThemeIdsByColorScheme
  }
): FrontendThemePreferences {
  const stored = isRecord(value) ? value : {}
  // Version 1 stored only the system/light/dark mode under themePreference.
  const legacyPreference = stored.themePreference
  const preferenceCandidate = stored.colorSchemePreference ?? legacyPreference

  return {
    colorSchemePreference: isColorSchemePreference(preferenceCandidate)
      ? preferenceCandidate
      : defaults.colorSchemePreference,
    themeIdsByColorScheme: isRecord(stored.themeIdsByColorScheme)
      ? normalizeThemeIdsByColorScheme(stored.themeIdsByColorScheme, defaults.themeIdsByColorScheme)
      : normalizeThemeIdsByColorScheme(defaults.themeIdsByColorScheme)
  }
}

export function resolveFrontendTheme(
  preferences: FrontendThemePreferences,
  systemColorScheme: ColorScheme
): ResolvedFrontendTheme {
  const colorScheme =
    preferences.colorSchemePreference === 'system'
      ? systemColorScheme
      : preferences.colorSchemePreference
  const themeId = normalizeThemeIdForColorScheme(
    colorScheme,
    preferences.themeIdsByColorScheme[colorScheme]
  )

  return {
    colorScheme,
    theme: getFrontendTheme(themeId),
    themeId
  }
}
