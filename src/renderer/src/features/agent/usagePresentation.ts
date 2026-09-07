/** Format cache hits as a percentage of recorded input; missing or inconsistent counters have no rate. */
export function formatCacheHitRate(
  cachedInputTokens: unknown,
  inputTokens: unknown
): string | null {
  if (
    typeof cachedInputTokens !== 'number' ||
    typeof inputTokens !== 'number' ||
    !Number.isFinite(cachedInputTokens) ||
    !Number.isFinite(inputTokens) ||
    inputTokens <= 0 ||
    cachedInputTokens < 0 ||
    cachedInputTokens > inputTokens
  ) {
    return null
  }

  return `${((cachedInputTokens / inputTokens) * 100).toFixed(2)}%`
}
