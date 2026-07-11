import { enUSTranslations } from './frontendTranslations.enUS'
import { zhCNTranslations } from './frontendTranslations.zhCN'

export type AppLanguage = 'zh-CN' | 'en-US'

export type TranslationKey = keyof typeof zhCNTranslations

export const translations: Record<AppLanguage, Record<TranslationKey, string>> = {
  'zh-CN': zhCNTranslations,
  'en-US': enUSTranslations
}
