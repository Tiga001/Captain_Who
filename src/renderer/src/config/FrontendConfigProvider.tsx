import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState
} from 'react'
import type { ReactNode } from 'react'
import {
  FRONTEND_CONFIG_STORAGE_KEY,
  frontendConfig,
  getFrontendCssVariables
} from './frontendConfig'
import { isThemeIdForColorScheme } from './frontendTheme'
import {
  FRONTEND_THEME_PREFERENCES_VERSION,
  normalizeFrontendThemePreferences,
  resolveFrontendTheme
} from './frontendThemePreferences'
import { getLanguageDefinition, getTranslation, isAppLanguage } from './languageRegistry'
import type {
  ColorScheme,
  ColorSchemePreference,
  FrontendThemeId,
  ThemeIdsByColorScheme
} from './frontendTheme'
import type { FrontendThemePreferences } from './frontendThemePreferences'
import type { AppLanguage, TranslationKey } from './frontendTranslations'
import { hostClient } from '../host/hostClient'

interface NormalizedFrontendConfig extends FrontendThemePreferences {
  language: AppLanguage
}

interface FrontendConfigContextValue {
  colorSchemePreference: ColorSchemePreference
  language: AppLanguage
  resolvedColorScheme: ColorScheme
  resolvedThemeId: FrontendThemeId
  setColorSchemePreference: (preference: ColorSchemePreference) => void
  setLanguage: (language: AppLanguage) => void
  setThemeForColorScheme: (colorScheme: ColorScheme, themeId: FrontendThemeId) => void
  t: (key: TranslationKey) => string
  themeIdsByColorScheme: ThemeIdsByColorScheme
}

const FrontendConfigContext = createContext<FrontendConfigContextValue | null>(null)

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function getSystemColorScheme(): ColorScheme {
  if (!window.matchMedia) return 'light'
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

function getDefaultFrontendConfig(): NormalizedFrontendConfig {
  return {
    colorSchemePreference: frontendConfig.colorSchemePreference,
    language: frontendConfig.language,
    themeIdsByColorScheme: frontendConfig.themeIdsByColorScheme
  }
}

function normalizeStoredFrontendConfig(value: unknown): NormalizedFrontendConfig {
  const defaults = getDefaultFrontendConfig()
  const stored = isRecord(value) ? value : {}
  const storedLanguage = typeof stored.language === 'string' ? stored.language : undefined
  const themePreferences = normalizeFrontendThemePreferences(stored, defaults)

  return {
    ...themePreferences,
    language: isAppLanguage(storedLanguage) ? storedLanguage : defaults.language
  }
}

function readStoredConfig(): NormalizedFrontendConfig {
  try {
    const rawValue = window.localStorage.getItem(FRONTEND_CONFIG_STORAGE_KEY)
    return normalizeStoredFrontendConfig(rawValue ? JSON.parse(rawValue) : undefined)
  } catch {
    return getDefaultFrontendConfig()
  }
}

export function FrontendConfigProvider({ children }: { children: ReactNode }) {
  const [initialConfig] = useState(() => readStoredConfig())
  const [language, setLanguage] = useState<AppLanguage>(initialConfig.language)
  const [systemColorScheme, setSystemColorScheme] = useState<ColorScheme>(() =>
    getSystemColorScheme()
  )
  const [colorSchemePreference, setColorSchemePreference] = useState<ColorSchemePreference>(
    initialConfig.colorSchemePreference
  )
  const [themeIdsByColorScheme, setThemeIdsByColorScheme] = useState<ThemeIdsByColorScheme>(
    initialConfig.themeIdsByColorScheme
  )
  const resolvedTheme = useMemo(
    () =>
      resolveFrontendTheme(
        {
          colorSchemePreference,
          themeIdsByColorScheme
        },
        systemColorScheme
      ),
    [colorSchemePreference, systemColorScheme, themeIdsByColorScheme]
  )

  const setThemeForColorScheme = useCallback(
    (colorScheme: ColorScheme, themeId: FrontendThemeId) => {
      if (!isThemeIdForColorScheme(themeId, colorScheme)) return

      setThemeIdsByColorScheme((current) =>
        current[colorScheme] === themeId ? current : { ...current, [colorScheme]: themeId }
      )
    },
    []
  )

  useLayoutEffect(() => {
    const variables = getFrontendCssVariables(frontendConfig, resolvedTheme.theme.tokens)
    Object.entries(variables).forEach(([name, value]) => {
      document.documentElement.style.setProperty(name, value)
    })

    document.documentElement.dataset.theme = resolvedTheme.themeId
    document.documentElement.dataset.colorScheme = resolvedTheme.colorScheme
    document.documentElement.dataset.colorSchemePreference = colorSchemePreference
    document.documentElement.style.colorScheme = resolvedTheme.colorScheme
  }, [colorSchemePreference, resolvedTheme])

  useLayoutEffect(() => {
    const definition = getLanguageDefinition(language)
    document.documentElement.lang = language
    document.documentElement.dir = definition.direction
  }, [language])

  useEffect(() => {
    void hostClient.app.setNativeThemeSource(colorSchemePreference)
  }, [colorSchemePreference])

  useEffect(() => {
    if (!window.matchMedia) return

    const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
    const handleThemeChange = () => {
      setSystemColorScheme(mediaQuery.matches ? 'dark' : 'light')
    }

    handleThemeChange()
    mediaQuery.addEventListener('change', handleThemeChange)
    return () => mediaQuery.removeEventListener('change', handleThemeChange)
  }, [])

  useEffect(() => {
    window.localStorage.setItem(
      FRONTEND_CONFIG_STORAGE_KEY,
      JSON.stringify({
        version: FRONTEND_THEME_PREFERENCES_VERSION,
        language,
        colorSchemePreference,
        themeIdsByColorScheme
      })
    )
  }, [colorSchemePreference, language, themeIdsByColorScheme])

  const value = useMemo<FrontendConfigContextValue>(
    () => ({
      colorSchemePreference,
      language,
      resolvedColorScheme: resolvedTheme.colorScheme,
      resolvedThemeId: resolvedTheme.themeId,
      setColorSchemePreference,
      setLanguage,
      setThemeForColorScheme,
      t: (key) => getTranslation(language, key),
      themeIdsByColorScheme
    }),
    [
      colorSchemePreference,
      language,
      resolvedTheme.colorScheme,
      resolvedTheme.themeId,
      setThemeForColorScheme,
      themeIdsByColorScheme
    ]
  )

  return <FrontendConfigContext.Provider value={value}>{children}</FrontendConfigContext.Provider>
}

export function useFrontendConfig() {
  const context = useContext(FrontendConfigContext)

  if (!context) {
    throw new Error('useFrontendConfig must be used within FrontendConfigProvider')
  }

  return context
}
