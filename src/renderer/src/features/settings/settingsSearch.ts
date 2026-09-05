import { settingsText, type SettingsNode, type SettingsTranslate } from './settingsDefinition'

export interface SearchableSettingsPage {
  id: string
  labelKey: Parameters<SettingsTranslate>[0]
  nodes: readonly SettingsNode[]
}

export interface SettingsSearchResult {
  id: string
  page: string
  label: string
  description: string
  path: string[]
  terms: string[]
  view?: string
  prerequisiteId?: string
  ancestorIds?: readonly string[]
}

export function buildSettingsSearchIndex(
  pages: readonly SearchableSettingsPage[],
  t: SettingsTranslate
): SettingsSearchResult[] {
  const entries: SettingsSearchResult[] = []
  for (const page of pages) {
    const ids = new Set<string>()
    const visit = (
      nodes: readonly SettingsNode[],
      path: string[],
      ancestorIds: string[],
      view?: string,
      prerequisiteId?: string
    ) => {
      for (const node of nodes) {
        if (ids.has(node.id)) throw new Error(`Duplicate setting ID: ${page.id}/${node.id}`)
        ids.add(node.id)
        if (node.supported && !node.supported()) continue
        const label = settingsText(node.title, t)
        const nodeView = node.view ?? view
        if (node.searchable ?? !node.children?.length) {
          entries.push({
            id: node.id,
            page: page.id,
            label,
            description: node.description ? settingsText(node.description, t) : '',
            path,
            terms: node.terms?.map((term) => settingsText(term, t)) ?? [],
            view: nodeView,
            prerequisiteId: node.prerequisiteId ?? prerequisiteId,
            ancestorIds
          })
        }
        if (node.children)
          visit(
            node.children,
            [...path, label],
            [node.id, ...ancestorIds],
            nodeView,
            node.prerequisiteId ?? prerequisiteId
          )
      }
    }
    visit(page.nodes, [t(page.labelKey)], [])
  }
  return entries
}

function normalize(value: string): string {
  return value.normalize('NFKC').toLocaleLowerCase().replace(/\s+/gu, ' ').trim()
}

/** Local metadata only: typing never mounts another page or reads configuration values. */
export function searchSettings(
  index: readonly SettingsSearchResult[],
  query: string
): SettingsSearchResult[] {
  const normalized = normalize(query)
  if (!normalized) return []
  const words = normalized.split(' ')
  return index
    .map((entry, order) => {
      const title = normalize(entry.label)
      const terms = normalize(entry.terms.join(' '))
      const description = normalize(entry.description)
      const path = normalize(entry.path.join(' '))
      const content = `${title} ${terms} ${description} ${path}`
      if (!words.every((word) => content.includes(word))) return { entry, order, score: 0 }
      const score =
        (title === normalized ? 200 : title.includes(normalized) ? 100 : 0) +
        words.reduce(
          (total, word) =>
            total +
            (title.includes(word)
              ? 40
              : terms.includes(word)
                ? 25
                : description.includes(word)
                  ? 10
                  : 1),
          0
        )
      return { entry, order, score }
    })
    .filter(({ score }) => score > 0)
    .sort((a, b) => b.score - a.score || a.order - b.order)
    .map(({ entry }) => entry)
}
