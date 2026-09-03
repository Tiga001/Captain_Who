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
  { label: '简体中文（中国）', value: 'zh-CN' },
  { label: '繁體中文（中國）', value: 'zh-TW' },
  { label: 'English (United States)', value: 'en-US' },
  { label: 'English (United Kingdom)', value: 'en-GB' },
  { label: '한국어', value: 'ko-KR' },
  { label: '日本語', value: 'ja-JP' },
  { label: 'Français', value: 'fr-FR' },
  { label: 'Italiano', value: 'it-IT' },
  { label: 'Русский', value: 'ru-RU' }
] as const

const SYSTEM_NOTIFICATION_TEMPLATE_KEYS = [
  'notification.system.scheduledTask',
  'notification.system.taskCompletedTitle',
  'notification.system.taskFailedTitle',
  'notification.system.taskCancelledTitle',
  'notification.system.approvalRequiredTitle',
  'notification.system.taskUpdateTitle',
  'notification.system.taskCompletedWithSubject',
  'notification.system.taskFailedWithSubject',
  'notification.system.taskCancelledWithSubject',
  'notification.system.approvalRequiredWithSubject',
  'notification.system.taskUpdateWithSubject',
  'notification.system.taskCompletedHidden',
  'notification.system.taskFailedHidden',
  'notification.system.taskCancelledHidden',
  'notification.system.approvalRequiredHidden',
  'notification.system.automationCompletedTitle',
  'notification.system.automationFailedTitle',
  'notification.system.automationCancelledTitle',
  'notification.system.automationApprovalRequiredTitle',
  'notification.system.automationImportantUpdateTitle',
  'notification.system.automationConfigurationBlockedTitle',
  'notification.system.automationUpdateTitle',
  'notification.system.batchApprovalRequired',
  'notification.system.batchConfigurationBlocked',
  'notification.system.batchFailed',
  'notification.system.batchImportantUpdate',
  'notification.system.batchCancelled',
  'notification.system.batchCompleted',
  'notification.system.batchCompletedTitle',
  'notification.system.batchUpdatesTitle'
] as const satisfies readonly TranslationKey[]

const ORDINARY_NOTIFICATION_SETTING_KEYS = [
  'notification.ordinaryModeLabel',
  'notification.ordinaryModeDescription',
  'notification.ordinaryModeAria',
  'notification.ordinaryModeNever',
  'notification.ordinaryModeAll',
  'notification.ordinaryModeNecessary',
  'notification.ordinaryModeCustom',
  'notification.ordinaryModeNeverDescription',
  'notification.ordinaryModeAllDescription',
  'notification.ordinaryModeNecessaryDescription',
  'notification.ordinaryModeCustomDescription',
  'notification.ordinaryCustomAria'
] as const satisfies readonly TranslationKey[]

type SystemNotificationTemplateKey = (typeof SYSTEM_NOTIFICATION_TEMPLATE_KEYS)[number]

const EXPECTED_SYSTEM_NOTIFICATION_PLACEHOLDERS = {
  'notification.system.scheduledTask': [],
  'notification.system.taskCompletedTitle': [],
  'notification.system.taskFailedTitle': [],
  'notification.system.taskCancelledTitle': [],
  'notification.system.approvalRequiredTitle': [],
  'notification.system.taskUpdateTitle': [],
  'notification.system.taskCompletedWithSubject': ['{subject}'],
  'notification.system.taskFailedWithSubject': ['{subject}'],
  'notification.system.taskCancelledWithSubject': ['{subject}'],
  'notification.system.approvalRequiredWithSubject': ['{subject}'],
  'notification.system.taskUpdateWithSubject': ['{subject}'],
  'notification.system.taskCompletedHidden': [],
  'notification.system.taskFailedHidden': [],
  'notification.system.taskCancelledHidden': [],
  'notification.system.approvalRequiredHidden': [],
  'notification.system.automationCompletedTitle': ['{subject}'],
  'notification.system.automationFailedTitle': ['{subject}'],
  'notification.system.automationCancelledTitle': ['{subject}'],
  'notification.system.automationApprovalRequiredTitle': ['{subject}'],
  'notification.system.automationImportantUpdateTitle': ['{subject}'],
  'notification.system.automationConfigurationBlockedTitle': ['{subject}'],
  'notification.system.automationUpdateTitle': ['{subject}'],
  'notification.system.batchApprovalRequired': ['{count}'],
  'notification.system.batchConfigurationBlocked': ['{count}'],
  'notification.system.batchFailed': ['{count}'],
  'notification.system.batchImportantUpdate': ['{count}'],
  'notification.system.batchCancelled': ['{count}'],
  'notification.system.batchCompleted': ['{count}'],
  'notification.system.batchCompletedTitle': ['{count}'],
  'notification.system.batchUpdatesTitle': ['{count}']
} as const satisfies Record<SystemNotificationTemplateKey, readonly string[]>

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

  it('keeps the complete typed system-notification template contract in every catalog', () => {
    const canonicalSystemKeys = Object.keys(languageRegistry['zh-CN'].translations)
      .filter((key): key is SystemNotificationTemplateKey => key.startsWith('notification.system.'))
      .sort()
    expect(canonicalSystemKeys).toEqual([...SYSTEM_NOTIFICATION_TEMPLATE_KEYS].sort())
    expect(canonicalSystemKeys).toHaveLength(30)

    for (const language of Object.keys(languageRegistry) as AppLanguage[]) {
      for (const key of SYSTEM_NOTIFICATION_TEMPLATE_KEYS) {
        expect(
          placeholders(languageRegistry[language].translations[key]),
          `${language}:${key}`
        ).toEqual(EXPECTED_SYSTEM_NOTIFICATION_PLACEHOLDERS[key])
      }
    }
  })

  it('keeps the ordinary-task notification mode contract translated in every catalog', () => {
    const canonicalOrdinaryNotificationKeys = Object.keys(languageRegistry['zh-CN'].translations)
      .filter((key): key is (typeof ORDINARY_NOTIFICATION_SETTING_KEYS)[number] =>
        key.startsWith('notification.ordinary')
      )
      .sort()
    expect(canonicalOrdinaryNotificationKeys).toEqual(
      [...ORDINARY_NOTIFICATION_SETTING_KEYS].sort()
    )

    for (const language of Object.keys(languageRegistry) as AppLanguage[]) {
      for (const key of ORDINARY_NOTIFICATION_SETTING_KEYS) {
        expect(languageRegistry[language].translations[key].trim(), `${language}:${key}`).not.toBe(
          ''
        )
        expect(
          placeholders(languageRegistry[language].translations[key]),
          `${language}:${key}`
        ).toEqual([])
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

  it('uses the current Classic name for the built-in classic themes', () => {
    for (const language of Object.keys(languageRegistry) as AppLanguage[]) {
      expect(getTranslation(language, 'appearance.themeVariant.classicLight')).toBe('Classic')
      expect(getTranslation(language, 'appearance.themeVariant.classicDark')).toBe('Classic')
    }
  })
})
