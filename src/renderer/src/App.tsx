import { FrontendConfigProvider } from './config/FrontendConfigProvider'
import { ModelSettingsProvider } from './config/ModelSettingsProvider'
import { ProjectSettingsProvider } from './config/ProjectSettingsProvider'
import { AppShell } from './app/AppShell'
import { ImagePreviewProvider } from './features/chat/components/ImagePreview'
import { ToastProvider } from './components/toast/ToastProvider'
import { AppStartupGate } from './features/startup/AppStartupGate'
import { AppStartupProvider } from './features/startup/AppStartupProvider'
import { AccountAuthProvider } from './features/auth/AccountAuthProvider'
import { useEffect, useState } from 'react'
import { hostClient } from './host/hostClient'

function HostWorkspace(): React.JSX.Element | null {
  const [ready, setReady] = useState(false)
  useEffect(() => {
    let cancelled = false
    void hostClient.app
      .whenReady()
      .then(() => {
        if (!cancelled) setReady(true)
      })
      .catch(() => undefined) // AppStartupProvider reports the same failure on the startup surface.
    return () => {
      cancelled = true
    }
  }, [])
  if (!ready) return null
  return (
    <ModelSettingsProvider>
      <ProjectSettingsProvider>
        <AppShell />
      </ProjectSettingsProvider>
    </ModelSettingsProvider>
  )
}

function App(): React.JSX.Element {
  return (
    <FrontendConfigProvider>
      <ToastProvider>
        <ImagePreviewProvider>
          <AccountAuthProvider>
            <AppStartupProvider>
              <AppStartupGate>
                <HostWorkspace />
              </AppStartupGate>
            </AppStartupProvider>
          </AccountAuthProvider>
        </ImagePreviewProvider>
      </ToastProvider>
    </FrontendConfigProvider>
  )
}

export default App
