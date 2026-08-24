// Renderer localization tests: keep every registered catalog structurally compatible.

import { describe, expect, it } from 'vitest'
import {
  appLanguageOptions,
  getTranslation,
  isAppLanguage,
  languageRegistry
} from '../../config/languageRegistry'
import type { AppLanguage, TranslationKey } from '../../config/languageRegistry'

const EXPECTED_LANGUAGE_OPTIONS = [
  { label: '中文（中国）', value: 'zh-CN' },
  { label: '繁體中文', value: 'zh-TW' },
  { label: 'English (United States)', value: 'en-US' },
  { label: 'English (United Kingdom)', value: 'en-GB' },
  { label: '한국어', value: 'ko-KR' },
  { label: '日本語', value: 'ja-JP' },
  { label: 'Français', value: 'fr-FR' },
  { label: 'Italiano', value: 'it-IT' },
  { label: 'Русский', value: 'ru-RU' }
] as const

function placeholders(value: string): string[] {
  return [...value.matchAll(/\{[^{}]+\}/g)].map(([placeholder]) => placeholder).sort()
}

describe('languageRegistry', () => {
  it('exposes every supported language in settings order', () => {
    expect(appLanguageOptions).toEqual(EXPECTED_LANGUAGE_OPTIONS)
    for (const { value } of EXPECTED_LANGUAGE_OPTIONS) expect(isAppLanguage(value)).toBe(true)
    expect(isAppLanguage('unsupported')).toBe(false)
  })

  it('keeps every catalog complete and preserves interpolation placeholders', () => {
    const canonicalKeys = Object.keys(
      languageRegistry['zh-CN'].translations
    ).sort() as TranslationKey[]

    for (const language of Object.keys(languageRegistry) as AppLanguage[]) {
      const translations = languageRegistry[language].translations
      const placeholderBaseline = language === 'zh-CN' || language === 'zh-TW' ? 'zh-CN' : 'en-US'
      expect(Object.keys(translations).sort()).toEqual(canonicalKeys)

      for (const key of canonicalKeys) {
        expect(translations[key].trim(), `${language}:${key}`).not.toBe('')
        expect(placeholders(translations[key]), `${language}:${key}`).toEqual(
          placeholders(languageRegistry[placeholderBaseline].translations[key])
        )
      }
    }
  })

  it('uses locale-specific wording instead of silently falling back', () => {
    for (const language of ['zh-TW', 'ko-KR', 'ja-JP', 'fr-FR', 'it-IT', 'ru-RU'] as const) {
      expect(getTranslation(language, 'settings.page.general')).not.toBe(
        getTranslation('en-US', 'settings.page.general')
      )
    }
    expect(getTranslation('en-GB', 'chat.favorite')).toBe('Favourite')
  })
})
