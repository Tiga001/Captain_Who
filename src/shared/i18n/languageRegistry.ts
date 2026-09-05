import { enGBTranslations } from './frontendTranslations.enGB'
import { enUSTranslations } from './frontendTranslations.enUS'
import { frFRTranslations } from './frontendTranslations.frFR'
import { itITTranslations } from './frontendTranslations.itIT'
import { jaJPTranslations } from './frontendTranslations.jaJP'
import { koKRTranslations } from './frontendTranslations.koKR'
import { ruRUTranslations } from './frontendTranslations.ruRU'
import { zhCNTranslations } from './frontendTranslations.zhCN'
import { zhTWTranslations } from './frontendTranslations.zhTW'
import { humanInteractionErrorTranslations } from './humanInteractionErrorTranslations'
import { humanInteractionHistoryTranslations } from './humanInteractionHistoryTranslations'
import { humanInteractionPanelTranslations } from './humanInteractionPanelTranslations'
import { humanInteractionSettingsTranslations } from './humanInteractionSettingsTranslations'

export type TranslationKey =
  | keyof typeof zhCNTranslations
  | keyof (typeof humanInteractionSettingsTranslations)['zh-CN']
  | keyof (typeof humanInteractionHistoryTranslations)['zh-CN']
  | keyof (typeof humanInteractionPanelTranslations)['zh-CN']
  | keyof (typeof humanInteractionErrorTranslations)['zh-CN']
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
    translations: {
      ...zhCNTranslations,
      ...humanInteractionSettingsTranslations['zh-CN'],
      ...humanInteractionHistoryTranslations['zh-CN'],
      ...humanInteractionPanelTranslations['zh-CN'],
      ...humanInteractionErrorTranslations['zh-CN']
    }
  },
  'zh-TW': {
    direction: 'ltr',
    displayName: '繁體中文（中國）',
    translations: {
      ...zhTWTranslations,
      ...humanInteractionSettingsTranslations['zh-TW'],
      ...humanInteractionHistoryTranslations['zh-TW'],
      ...humanInteractionPanelTranslations['zh-TW'],
      ...humanInteractionErrorTranslations['zh-TW']
    }
  },
  'en-US': {
    direction: 'ltr',
    displayName: 'English (United States)',
    translations: {
      ...enUSTranslations,
      ...humanInteractionSettingsTranslations['en-US'],
      ...humanInteractionHistoryTranslations['en-US'],
      ...humanInteractionPanelTranslations['en-US'],
      ...humanInteractionErrorTranslations['en-US']
    }
  },
  'en-GB': {
    direction: 'ltr',
    displayName: 'English (United Kingdom)',
    translations: {
      ...enGBTranslations,
      ...humanInteractionSettingsTranslations['en-GB'],
      ...humanInteractionHistoryTranslations['en-GB'],
      ...humanInteractionPanelTranslations['en-GB'],
      ...humanInteractionErrorTranslations['en-GB']
    }
  },
  'ko-KR': {
    direction: 'ltr',
    displayName: '한국어',
    translations: {
      ...koKRTranslations,
      ...humanInteractionSettingsTranslations['ko-KR'],
      ...humanInteractionHistoryTranslations['ko-KR'],
      ...humanInteractionPanelTranslations['ko-KR'],
      ...humanInteractionErrorTranslations['ko-KR']
    }
  },
  'ja-JP': {
    direction: 'ltr',
    displayName: '日本語',
    translations: {
      ...jaJPTranslations,
      ...humanInteractionSettingsTranslations['ja-JP'],
      ...humanInteractionHistoryTranslations['ja-JP'],
      ...humanInteractionPanelTranslations['ja-JP'],
      ...humanInteractionErrorTranslations['ja-JP']
    }
  },
  'fr-FR': {
    direction: 'ltr',
    displayName: 'Français',
    translations: {
      ...frFRTranslations,
      ...humanInteractionSettingsTranslations['fr-FR'],
      ...humanInteractionHistoryTranslations['fr-FR'],
      ...humanInteractionPanelTranslations['fr-FR'],
      ...humanInteractionErrorTranslations['fr-FR']
    }
  },
  'it-IT': {
    direction: 'ltr',
    displayName: 'Italiano',
    translations: {
      ...itITTranslations,
      ...humanInteractionSettingsTranslations['it-IT'],
      ...humanInteractionHistoryTranslations['it-IT'],
      ...humanInteractionPanelTranslations['it-IT'],
      ...humanInteractionErrorTranslations['it-IT']
    }
  },
  'ru-RU': {
    direction: 'ltr',
    displayName: 'Русский',
    translations: {
      ...ruRUTranslations,
      ...humanInteractionSettingsTranslations['ru-RU'],
      ...humanInteractionHistoryTranslations['ru-RU'],
      ...humanInteractionPanelTranslations['ru-RU'],
      ...humanInteractionErrorTranslations['ru-RU']
    }
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
