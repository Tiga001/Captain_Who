// Renderer UI translation registry.
import { enUSTranslations } from './frontendTranslations.enUS'
import { zhCNTranslations } from './frontendTranslations.zhCN'

export type AppLanguage = 'zh-CN' | 'en-US'

export const languageOptions: Array<{ value: AppLanguage; label: string }> = [
  { value: 'zh-CN', label: '中文' },
  { value: 'en-US', label: 'English' }
]

export { enUSTranslations, zhCNTranslations }

export type TranslationKey = keyof typeof zhCNTranslations

export const translations: Record<AppLanguage, Record<TranslationKey, string>> = {
  'zh-CN': zhCNTranslations,
  'en-US': enUSTranslations
}
