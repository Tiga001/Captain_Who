// Renderer UI layer: keeps the main workspace mounted while full-screen app views cover it.
import { forwardRef, type ComponentPropsWithoutRef } from 'react'

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
