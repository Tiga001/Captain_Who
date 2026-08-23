// Renderer UI layer: keeps the main workspace mounted while full-screen app views cover it.
import {
  createElement,
  forwardRef,
  type ComponentPropsWithoutRef,
  type HTMLAttributes,
  type ReactNode
} from 'react'

export type PrimaryView = 'conversation' | 'scheduled'

export function getVisibleActiveConversationId(
  primaryView: PrimaryView,
  activeConversationId: string | null
): string | null {
  return primaryView === 'conversation' ? activeConversationId : null
}

interface AppShellWorkspaceProps extends ComponentPropsWithoutRef<'div'> {
  settingsOpen: boolean
}

export const AppShellWorkspace = forwardRef<HTMLDivElement, AppShellWorkspaceProps>(
  function AppShellWorkspace({ settingsOpen, ...props }, ref) {
    return (
      <div
        {...props}
        ref={ref}
        aria-hidden={settingsOpen ? true : undefined}
        data-settings-open={settingsOpen ? 'true' : undefined}
        inert={settingsOpen ? true : undefined}
      />
    )
  }
)

interface AppShellCoveredRegionProps extends HTMLAttributes<HTMLElement> {
  as: 'main' | 'aside'
  children?: ReactNode
  covered: boolean
}

/**
 * Makes a live workspace region unreachable while the Scheduled page visually covers it.
 * The element and all of its stateful children stay mounted; coverage is accessibility and
 * interaction isolation only.
 */
export function AppShellCoveredRegion({
  as,
  children,
  covered,
  ...props
}: AppShellCoveredRegionProps) {
  return createElement(
    as,
    {
      ...props,
      'aria-hidden': covered ? true : undefined,
      'data-scheduled-covered': covered ? 'true' : undefined,
      inert: covered ? true : undefined
    },
    children
  )
}
