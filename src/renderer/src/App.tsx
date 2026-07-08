// Renderer UI.
import { FrontendConfigProvider } from "./config/FrontendConfigProvider";
import { ModelSettingsProvider } from "./config/ModelSettingsProvider";
import { ProjectSettingsProvider } from "./config/ProjectSettingsProvider";
import { AppShell } from "./app/AppShell";
import { ImagePreviewProvider } from "./features/chat/components/ImagePreview";

function App(): React.JSX.Element {
  return (
    <FrontendConfigProvider>
      <ImagePreviewProvider>
        <ModelSettingsProvider>
          <ProjectSettingsProvider>
            <AppShell />
          </ProjectSettingsProvider>
        </ModelSettingsProvider>
      </ImagePreviewProvider>
    </FrontendConfigProvider>
  );
}

export default App;
