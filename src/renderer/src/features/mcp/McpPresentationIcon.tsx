import { Globe2, MousePointerClick, PlugZap } from 'lucide-react'

export function McpBuiltinCapabilityIcon({ capabilityId }: { capabilityId: string }) {
  if (capabilityId === 'browser_automation') {
    return (
      <span
        aria-hidden="true"
        className="mcp-presentation-icon mcp-presentation-icon--browser"
        data-mcp-icon="browser-automation"
      >
        <Globe2 className="mcp-presentation-icon__globe" />
        <MousePointerClick className="mcp-presentation-icon__pointer" />
      </span>
    )
  }

  return <McpExternalServerIcon />
}

export function McpExternalServerIcon() {
  return (
    <span aria-hidden="true" className="mcp-presentation-icon" data-mcp-icon="external-server">
      <PlugZap />
    </span>
  )
}
