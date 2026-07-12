export function formatContextWindow(tokens?: number): string {
  if (!Number.isSafeInteger(tokens) || !tokens || tokens < 1) return '—'

  const formatScaled = (divisor: number, suffix: string): string => {
    const value = (tokens / divisor).toFixed(2).replace(/\.?0+$/, '')
    return `${value}${suffix}`
  }

  if (tokens >= 1_000_000) return formatScaled(1_000_000, 'M')
  if (tokens >= 1_000) return formatScaled(1_000, 'K')
  return String(tokens)
}
