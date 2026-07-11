import { FrontendConfigProvider } from './config/FrontendConfigProvider'
import { ModelSettingsProvider } from './config/ModelSettingsProvider'
import { ProjectSettingsProvider } from './config/ProjectSettingsProvider'
import { AppShell } from './app/AppShell'
import { ImagePreviewProvider } from './features/chat/components/ImagePreview'
import { ToastProvider } from './components/toast/ToastProvider'

function App(): React.JSX.Element {
  return (
    <FrontendConfigProvider>
      <ToastProvider>
        <ImagePreviewProvider>
          <ModelSettingsProvider>
            <ProjectSettingsProvider>
              <AppShell />
            </ProjectSettingsProvider>
          </ModelSettingsProvider>
        </ImagePreviewProvider>
      </ToastProvider>
    </FrontendConfigProvider>
  )
}

export default App
