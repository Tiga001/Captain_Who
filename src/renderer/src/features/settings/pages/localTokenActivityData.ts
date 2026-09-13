const DAY_MS = 86_400_000

export type TokenPoint = { date: string; tokens: bigint }

export function shanghaiDate(timestamp = Date.now()): string {
  const parts = new Intl.DateTimeFormat('en-CA', {
    timeZone: 'Asia/Shanghai',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit'
  }).formatToParts(timestamp)
  const part = (type: string) => parts.find((item) => item.type === type)!.value
  return `${part('year')}-${part('month')}-${part('day')}`
}

export function parseTokenCount(value: string): bigint {
  return /^\d+$/.test(value) ? BigInt(value) : BigInt(0)
}

export function buildTokenYear(
  year: number,
  days: { date: string; tokenCount: string }[]
): TokenPoint[] {
  const byDate = new Map(days.map((day) => [day.date, parseTokenCount(day.tokenCount)]))
  const points: TokenPoint[] = []
  const end = Date.UTC(year + 1, 0, 1)
  for (let time = Date.UTC(year, 0, 1); time < end; time += DAY_MS) {
    const date = new Date(time).toISOString().slice(0, 10)
    points.push({ date, tokens: byDate.get(date) ?? BigInt(0) })
  }
  return points
}

export function tokenIntensity(value: bigint, peak: bigint): number {
  if (value === BigInt(0) || peak === BigInt(0)) return 0
  return Math.min(4, Number((value * BigInt(4) - BigInt(1)) / peak) + 1)
}
