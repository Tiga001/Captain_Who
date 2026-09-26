export function projectWorkflowText(language: string) {
  const zh = language === 'zh-CN'
  return (cn: string, en: string) => (zh ? cn : en)
}

export const WORKFLOW_CONVERSATION_DRAG_TYPE = 'application/x-captain-workflow-conversation'

export const WORKFLOW_COLORS = [
  '#4F8FEA',
  '#B57BE8',
  '#28AA91',
  '#ECA54E',
  '#E6799D',
  '#6A9D55',
  '#8879D8',
  '#D67E55'
] as const

export function pickUnusedWorkflowColor(colors: readonly string[]): string | null {
  const used = new Set(colors.map((color) => color.trim().toLowerCase()))
  return WORKFLOW_COLORS.find((color) => !used.has(color.toLowerCase())) ?? null
}
