// Renderer UI.
import { FrontendConfigProvider } from "./config/FrontendConfigProvider";
import { ModelSettingsProvider } from "./config/ModelSettingsProvider";
import { ProjectSettingsProvider } from "./config/ProjectSettingsProvider";
import { AppShell } from "./app/AppShell";

function App(): React.JSX.Element {
  return (
    <FrontendConfigProvider>
      <ModelSettingsProvider>
        <ProjectSettingsProvider>
          <AppShell />
        </ProjectSettingsProvider>
      </ModelSettingsProvider>
    </FrontendConfigProvider>
  );
}

export default App;
