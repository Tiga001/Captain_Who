import { enUSTranslations } from './frontendTranslations.enUS'
import { zhCNTranslations } from './frontendTranslations.zhCN'

export type TranslationKey = keyof typeof zhCNTranslations
export type LanguageDirection = 'ltr' | 'rtl'

type TranslationCatalog = Readonly<Record<TranslationKey, string>>

export interface LanguageDefinition {
  readonly direction: LanguageDirection
  readonly displayName: string
  readonly translations: TranslationCatalog
}

export const languageRegistry = {
  'zh-CN': {
    direction: 'ltr',
    displayName: '中文（中国）',
    translations: zhCNTranslations
  },
  'en-US': {
    direction: 'ltr',
    displayName: 'English (United States)',
    translations: enUSTranslations
  }
} as const satisfies Record<string, LanguageDefinition>

export type AppLanguage = keyof typeof languageRegistry

export interface AppLanguageOption {
  readonly label: string
  readonly value: AppLanguage
}

export const DEFAULT_APP_LANGUAGE: AppLanguage = 'zh-CN'

export const appLanguageOptions: ReadonlyArray<AppLanguageOption> = Object.freeze(
  (Object.keys(languageRegistry) as AppLanguage[]).map((value) => ({
    label: languageRegistry[value].displayName,
    value
  }))
)

export function isAppLanguage(value: unknown): value is AppLanguage {
  return typeof value === 'string' && Object.prototype.hasOwnProperty.call(languageRegistry, value)
}

export function getLanguageDefinition(language: AppLanguage): LanguageDefinition {
  return languageRegistry[language]
}

export function getTranslation(language: AppLanguage, key: TranslationKey): string {
  return languageRegistry[language].translations[key]
}
