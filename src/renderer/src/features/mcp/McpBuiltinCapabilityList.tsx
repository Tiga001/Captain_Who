import type { McpBuiltinCapabilityListItem } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { getBuiltinCapabilityDisplayName } from './builtinCapabilityPresentation'
import { McpBuiltinCapabilityIcon } from './McpPresentationIcon'
import { toSafeMcpDisplayText } from './mcpSafeDisplay'

interface McpBuiltinCapabilityListProps {
  capabilities: readonly McpBuiltinCapabilityListItem[]
  onSetAllowed: (capability: McpBuiltinCapabilityListItem, allowed: boolean) => void
  pendingCapabilities: ReadonlySet<McpBuiltinCapabilityListItem['capabilityId']>
}

export function McpBuiltinCapabilityList({
  capabilities,
  onSetAllowed,
  pendingCapabilities
}: McpBuiltinCapabilityListProps) {
  const { t } = useFrontendConfig()

  return (
    <div className="mcp-builtin-capability-list">
      {capabilities.map((capability) => {
        const pending = pendingCapabilities.has(capability.capabilityId)
        const name = getBuiltinCapabilityDisplayName(capability.capabilityId, t)
        const description = capabilityDescription(capability, t)
        return (
          <article
            aria-busy={pending || undefined}
            className="mcp-builtin-capability-row"
            key={capability.capabilityId}
          >
            <McpBuiltinCapabilityIcon capabilityId={capability.capabilityId} />
            <div className="mcp-builtin-capability-row__copy">
              <strong>{name}</strong>
              {description && <p>{description}</p>}
            </div>
            <div className="mcp-builtin-capability-row__actions">
              <button
                aria-checked={capability.userAllowed}
                aria-label={replaceToken(t('mcp.builtin.toggleNamed'), 'name', name)}
                className="settings-switch"
                data-state={capability.userAllowed ? 'on' : 'off'}
                disabled={pending}
                onClick={() => onSetAllowed(capability, !capability.userAllowed)}
                role="switch"
                type="button"
              >
                <span aria-hidden="true" className="settings-switch__thumb" />
              </button>
            </div>
          </article>
        )
      })}
    </div>
  )
}

type Translate = ReturnType<typeof useFrontendConfig>['t']

function capabilityDescription(capability: McpBuiltinCapabilityListItem, t: Translate): string {
  switch (capability.capabilityId) {
    case 'browser_automation':
      return t('mcp.builtin.browserAutomation.description')
    default:
      return toSafeMcpDisplayText(capability.description)
  }
}

function replaceToken(template: string, name: string, value: string): string {
  return template.replaceAll(`{${name}}`, value)
}
