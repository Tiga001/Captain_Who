import { cloneElement, type ReactElement } from 'react'
import type { TranslationKey } from '../../config/frontendTranslations'

export type SettingsText = TranslationKey | { readonly text: string }
export type SettingsTranslate = (key: TranslationKey) => string

/** Page-owned UI definitions. Search reads the same nodes that render the settings. */
export interface SettingsNode {
  readonly id: string
  readonly title: SettingsText
  readonly description?: SettingsText
  readonly terms?: readonly SettingsText[]
  readonly children?: readonly SettingsNode[]
  /** A presentation view, never an instruction to change a stored setting. */
  readonly view?: string
  /** Existing parent/selector to show when this field needs an editing context. */
  readonly prerequisiteId?: string
  readonly supported?: () => boolean
  readonly searchable?: boolean
}

export function defineSettingsNodes<const T extends readonly SettingsNode[]>(nodes: T): T {
  return nodes
}

export function settingsText(text: SettingsText, t: SettingsTranslate): string {
  return typeof text === 'string' ? t(text) : text.text
}

export function settingLabel(node: SettingsNode, t: SettingsTranslate): string {
  return settingsText(node.title, t)
}

export function settingDescription(node: SettingsNode, t: SettingsTranslate): string | undefined {
  return node.description ? settingsText(node.description, t) : undefined
}

/** No wrapper DOM: retain each page's original element, classes, controls and layout. */
export function renderSettingsNodes<T extends SettingsNode>(
  nodes: readonly T[],
  render: (node: T) => ReactElement | null
): Array<ReactElement | null> {
  return nodes.map((node) => {
    if (node.supported && !node.supported()) return null
    const element = render(node)
    if (!element) return null
    return cloneElement(element as ReactElement<Record<string, unknown>>, {
      key: node.id,
      ...(typeof element.type === 'string' ? { 'data-setting-id': node.id } : {})
    })
  })
}
