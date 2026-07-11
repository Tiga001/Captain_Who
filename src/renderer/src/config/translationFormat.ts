import type { TranslationKey } from './frontendTranslations'

export type Translate = (key: TranslationKey) => string

export function formatTranslation(
  t: Translate,
  key: TranslationKey,
  values: Record<string, string | number>
) {
  return Object.entries(values).reduce(
    (text, [name, value]) => text.split(`{${name}}`).join(String(value)),
    t(key)
  )
}
