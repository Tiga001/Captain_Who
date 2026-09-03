import { enGBTranslations } from './frontendTranslations.enGB'
import { enUSTranslations } from './frontendTranslations.enUS'
import { frFRTranslations } from './frontendTranslations.frFR'
import { itITTranslations } from './frontendTranslations.itIT'
import { jaJPTranslations } from './frontendTranslations.jaJP'
import { koKRTranslations } from './frontendTranslations.koKR'
import { ruRUTranslations } from './frontendTranslations.ruRU'
import { zhCNTranslations } from './frontendTranslations.zhCN'
import { zhTWTranslations } from './frontendTranslations.zhTW'

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
    displayName: '简体中文（中国）',
    translations: zhCNTranslations
  },
  'zh-TW': {
    direction: 'ltr',
    displayName: '繁體中文（中國）',
    translations: zhTWTranslations
  },
  'en-US': {
    direction: 'ltr',
    displayName: 'English (United States)',
    translations: enUSTranslations
  },
  'en-GB': {
    direction: 'ltr',
    displayName: 'English (United Kingdom)',
    translations: enGBTranslations
  },
  'ko-KR': {
    direction: 'ltr',
    displayName: '한국어',
    translations: koKRTranslations
  },
  'ja-JP': {
    direction: 'ltr',
    displayName: '日本語',
    translations: jaJPTranslations
  },
  'fr-FR': {
    direction: 'ltr',
    displayName: 'Français',
    translations: frFRTranslations
  },
  'it-IT': {
    direction: 'ltr',
    displayName: 'Italiano',
    translations: itITTranslations
  },
  'ru-RU': {
    direction: 'ltr',
    displayName: 'Русский',
    translations: ruRUTranslations
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
