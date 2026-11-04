import { vi } from 'vitest'
import { testState, frontendConfig } from './state'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => frontendConfig
}))

vi.mock('../../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: testState.enabledModels,
    models: testState.enabledModels
  })
}))

vi.mock('../../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({
    projects: testState.projects,
    deleteProject: testState.deleteProject,
    openCreateProjectDialog: vi.fn(async () => null),
    openEditProjectDialog: testState.openEditProjectDialog,
    showProjectInFolder: vi.fn(),
    showProjectFolder: vi.fn(),
    togglePinProject: vi.fn()
  })
}))

vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: testState.showToast })
}))

vi.mock('../../useShellLayout', () => ({
  useShellLayout: () => ({
    canOpenBottomPanel: testState.canOpenBottomPanel,
    openBottomPanel: testState.openBottomPanel,
    openRightSidebar: testState.openRightSidebar,
    setRightSidebarMaximized: testState.setRightSidebarMaximized,
    commitSidebarResize: vi.fn(),
    leftResizeMetrics: { maximum: 420, minimum: 220, width: 0 },
    leftOpen: false,
    leftWidth: 0,
    rightMaximized: testState.rightMaximized,
    rightOpen: testState.rightOpen,
    rightResizeMetrics: { maximum: 1200, minimum: 280, width: 0 },
    rightWidth: 0,
    shellRef: { current: null },
    toggleLeftSidebar: vi.fn(),
    toggleRightSidebar: vi.fn(),
    toggleRightSidebarMaximized: vi.fn()
  })
}))
