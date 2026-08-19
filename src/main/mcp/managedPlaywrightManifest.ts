import {
  MANAGED_PLAYWRIGHT_EXPOSED_TOOLS,
  MANAGED_PLAYWRIGHT_POLICY_MANIFEST,
  managedPlaywrightReviewedTool,
  type ManagedPlaywrightHandlingMode,
  type ManagedPlaywrightReviewedTool
} from './managedPlaywrightCatalog'

export const MANAGED_PLAYWRIGHT_PACKAGE_NAME = '@playwright/mcp'
export const MANAGED_PLAYWRIGHT_PACKAGE_VERSION = '0.0.79'
export const MANAGED_PLAYWRIGHT_MANIFEST_SCHEMA_VERSION = 2
// Stable Host-owned routing identity. This is deliberately unrelated to the user-visible name and
// cannot be supplied by Renderer or by the managed Server.
export const MANAGED_PLAYWRIGHT_SERVER_ID = 'b77b3d54-b7c6-4ead-9cbd-3b9fe50d5311'

export type ManagedPlaywrightToolSafety = 'read_only' | 'destructive'

export interface ManagedPlaywrightToolManifestEntry extends ManagedPlaywrightReviewedTool {
  readonly handlingMode: ManagedPlaywrightHandlingMode
}

export interface ManagedPlaywrightManifest {
  readonly schemaVersion: 2
  readonly packageName: typeof MANAGED_PLAYWRIGHT_PACKAGE_NAME
  readonly packageVersion: typeof MANAGED_PLAYWRIGHT_PACKAGE_VERSION
  readonly managedMcpId: 'builtin.browser_automation.mcp'
  readonly capabilityId: 'browser_automation'
  readonly manifestVersion: string
  readonly upstreamCatalogDigest: string
  readonly policyDigest: string
  readonly tools: readonly ManagedPlaywrightToolManifestEntry[]
}

export const MANAGED_PLAYWRIGHT_MANIFEST: ManagedPlaywrightManifest = Object.freeze({
  schemaVersion: 2,
  packageName: MANAGED_PLAYWRIGHT_PACKAGE_NAME,
  packageVersion: MANAGED_PLAYWRIGHT_PACKAGE_VERSION,
  managedMcpId: 'builtin.browser_automation.mcp',
  capabilityId: 'browser_automation',
  manifestVersion: MANAGED_PLAYWRIGHT_POLICY_MANIFEST.manifestVersion,
  upstreamCatalogDigest: MANAGED_PLAYWRIGHT_POLICY_MANIFEST.upstreamCatalogDigest,
  policyDigest: MANAGED_PLAYWRIGHT_POLICY_MANIFEST.policyDigest,
  tools: MANAGED_PLAYWRIGHT_EXPOSED_TOOLS
})

export function managedPlaywrightTool(
  rawName: string
): ManagedPlaywrightToolManifestEntry | undefined {
  const reviewed = managedPlaywrightReviewedTool(rawName)
  return reviewed?.exposed ? reviewed : undefined
}
