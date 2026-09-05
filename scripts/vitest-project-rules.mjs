const gitReviewTests = 'src/renderer/src/features/gitReview/__tests__'
const rightSidebarTests = 'src/renderer/src/features/rightSidebar/__tests__'
const bottomPanelTests = 'src/renderer/src/features/bottomPanel/__tests__'
const filesTests = 'src/renderer/src/features/files/__tests__'
const mainWindowLifecycleTest = 'src/main/mainWindowLifecycle.test.ts'
const workspaceFilesTests = 'src/main/workspaceFiles'
const coreMainTests = 'src/main/core'
const electronFixtureTests = `${coreMainTests}/**/*.electron.test.ts`
const managedPlaywrightElectronE2e = 'src/main/core/managedPlaywrightBridge.electron.test.ts'
const automationCoreE2e = 'src/main/core/automationHostRealCore.integration.test.ts'
const mcpMainTests = 'src/main/mcp'
const terminalMainTests = 'src/main/terminal'
const terminalPreloadTests = 'src/preload'
const terminalRendererTests = 'src/renderer/src/features/terminal/__tests__'
const skillsTests = 'src/renderer/src/features/skills/__tests__'
const mcpTests = 'src/renderer/src/features/mcp/__tests__'
const appTests = 'src/renderer/src/app/__tests__'
const chatTests = 'src/renderer/src/features/chat/__tests__'
const automationsTests = 'src/renderer/src/features/automations/__tests__'
const notificationsTests = 'src/renderer/src/features/notifications/__tests__'
const agentCollaborationTests = 'src/renderer/src/features/agentCollaboration'
const protocolTests = 'packages/protocol/src'

export const vitestCandidateTestGlobs = [
  'src/**/*.{test,spec}.{ts,tsx}',
  'packages/**/*.{test,spec}.{ts,tsx}'
]

export const vitestProjectFileRules = {
  unit: {
    include: [
      `${gitReviewTests}/**/*.test.ts`,
      `${rightSidebarTests}/**/*.test.ts`,
      mainWindowLifecycleTest,
      `${coreMainTests}/**/*.test.ts`,
      `${mcpMainTests}/**/*.test.ts`,
      `${workspaceFilesTests}/**/*.test.ts`,
      `${terminalMainTests}/**/*.test.ts`,
      `${terminalPreloadTests}/**/*.test.ts`,
      `${terminalRendererTests}/**/*.test.ts`,
      `${skillsTests}/**/*.test.ts`,
      `${mcpTests}/**/*.test.ts`,
      `${appTests}/**/*.test.ts`,
      `${chatTests}/**/*.test.ts`,
      `${automationsTests}/**/*.test.ts`,
      `${notificationsTests}/**/*.test.ts`,
      `${agentCollaborationTests}/**/*.test.ts`,
      `${protocolTests}/**/*.test.ts`
    ],
    exclude: [electronFixtureTests, automationCoreE2e]
  },
  browser: {
    include: [
      `${appTests}/**/*.browser.test.tsx`,
      `${chatTests}/**/*.browser.test.tsx`,
      `${automationsTests}/**/*.browser.test.tsx`,
      `${notificationsTests}/**/*.browser.test.tsx`,
      `${skillsTests}/**/*.browser.test.tsx`,
      `${mcpTests}/**/*.browser.test.tsx`,
      `${gitReviewTests}/**/*.browser.test.tsx`,
      `${rightSidebarTests}/**/*.browser.test.tsx`,
      `${bottomPanelTests}/**/*.browser.test.tsx`,
      `${terminalRendererTests}/**/*.browser.test.tsx`,
      `${filesTests}/**/*.browser.test.tsx`,
      `${agentCollaborationTests}/**/*.browser.test.tsx`
    ],
    exclude: []
  },
  'electron-fixtures': {
    include: [electronFixtureTests],
    exclude: [managedPlaywrightElectronE2e]
  },
  'managed-playwright-e2e': {
    include: [managedPlaywrightElectronE2e],
    exclude: []
  },
  'automation-core-e2e': {
    include: [automationCoreE2e],
    exclude: []
  }
}
