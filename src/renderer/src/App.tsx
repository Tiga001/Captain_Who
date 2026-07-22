import { FrontendConfigProvider } from './config/FrontendConfigProvider'
import { ModelSettingsProvider } from './config/ModelSettingsProvider'
import { ProjectSettingsProvider } from './config/ProjectSettingsProvider'
import { AppShell } from './app/AppShell'
import { ImagePreviewProvider } from './features/chat/components/ImagePreview'
import { ToastProvider } from './components/toast/ToastProvider'
import { AppStartupGate } from './features/startup/AppStartupGate'
import { AppStartupProvider } from './features/startup/AppStartupProvider'

function App(): React.JSX.Element {
  return (
    <FrontendConfigProvider>
      <ToastProvider>
        <ImagePreviewProvider>
          <AppStartupProvider>
            <ModelSettingsProvider>
              <ProjectSettingsProvider>
                <AppStartupGate>
                  <AppShell />
                </AppStartupGate>
              </ProjectSettingsProvider>
            </ModelSettingsProvider>
          </AppStartupProvider>
        </ImagePreviewProvider>
      </ToastProvider>
    </FrontendConfigProvider>
  )
}

export default App
