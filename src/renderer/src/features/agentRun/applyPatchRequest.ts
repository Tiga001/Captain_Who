function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/**
 * Returns the current model-visible apply_patch request. The root is deliberately exact so
 * Renderer projections never reinterpret retired flat arguments or mixed current/legacy shapes.
 */
export function getApplyPatchRequest(args: unknown): Record<string, unknown> | undefined {
  if (!isRecord(args) || Object.keys(args).length !== 1 || !('request' in args)) return undefined
  return isRecord(args.request) ? args.request : undefined
}
