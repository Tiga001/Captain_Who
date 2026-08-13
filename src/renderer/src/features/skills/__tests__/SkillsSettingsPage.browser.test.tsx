import { HostInvocationError } from '@mycopilot/host-api'
import type {
  SkillInstallationCommitOutput,
  SkillInstallationPreview,
  SkillManagementEntry,
  SkillSourceResolutionCandidate,
  SkillsCancelSourceResolutionOutput,
  SkillsListManagementOutput,
  SkillsResolveInstallationSourceOutput
} from '@mycopilot/protocol'
import { StrictMode, useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const service = vi.hoisted(() => ({
  cancelPreparation: vi.fn(),
  cancelSourceResolution: vi.fn(),
  commitInstallation: vi.fn(),
  inspectInstallation: vi.fn(),
  listManagement: vi.fn(),
  onChanged: vi.fn(),
  resolveInstallationSource: vi.fn(),
  selectInstallationDirectory: vi.fn(),
  setEnabled: vi.fn(),
  showToast: vi.fn(),
  uninstall: vi.fn(),
  unsubscribe: vi.fn()
}))

let changedHandler: (() => void) | undefined

vi.mock('../management/skillsManagementClient', () => ({
  cancelSkillPreparation: service.cancelPreparation,
  cancelSkillSourceResolution: service.cancelSourceResolution,
  commitSkillInstallation: service.commitInstallation,
  inspectSkillInstallation: service.inspectInstallation,
  listManagedSkills: service.listManagement,
  onManagedSkillsChanged: service.onChanged,
  resolveSkillInstallationSource: service.resolveInstallationSource,
  selectSkillInstallationDirectory: service.selectInstallationDirectory,
  setManagedSkillEnabled: service.setEnabled,
  uninstallManagedSkill: service.uninstall
}))

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

vi.mock('../../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], selectProjectDirectory: vi.fn() })
}))

vi.mock('../../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: service.showToast })
}))

vi.mock('../../storage/storageClient', () => ({
  getTranslucentSidebarOpacityPercent: () => 54
}))

vi.mock('../../settings/pages/AppearanceSettingsPage', () => ({
  AppearanceSettingsPage: () => <div>appearance-page</div>
}))
vi.mock('../../settings/pages/AgentTemplatesSettingsPage', () => ({
  AgentTemplatesSettingsPage: () => <div>agent-templates-page</div>
}))
vi.mock('../../settings/pages/ArchivedConversationsSettingsPage', () => ({
  ArchivedConversationsSettingsPage: () => <div>archived-page</div>
}))
vi.mock('../../settings/pages/ConfigurationSettingsPage', () => ({
  ConfigurationSettingsPage: () => <div>configuration-page</div>
}))
vi.mock('../../settings/pages/EnvironmentSettingsPage', () => ({
  EnvironmentSettingsPage: () => <div>environment-page</div>
}))
vi.mock('../../settings/pages/GeneralSettingsPage', () => ({
  GeneralSettingsPage: () => <div>general-page</div>
}))
vi.mock('../../settings/pages/PersonalizationSettingsPage', () => ({
  PersonalizationSettingsPage: () => <div>personalization-page</div>
}))
vi.mock('../../settings/pages/ProfileSettingsPage', () => ({
  ProfileSettingsPage: () => <div>profile-page</div>
}))
vi.mock('../../settings/pages/UsageBillingSettingsPage', () => ({
  UsageBillingSettingsPage: () => <div>usage-page</div>
}))
vi.mock('../../mcp/McpSettingsPage', () => ({
  McpSettingsPage: ({ onDirtyChange }: { onDirtyChange?: (dirty: boolean) => void }) => (
    <div>
      <span>mcp-page</span>
      <button onClick={() => onDirtyChange?.(true)} type="button">
        make-mcp-dirty
      </button>
    </div>
  )
}))

const [{ SkillsSettingsPage }, { SettingsPage }] = await Promise.all([
  import('../../settings/pages/SkillsSettingsPage'),
  import('../../settings/SettingsPage')
])

const bundledSkill: SkillManagementEntry = {
  actions: { canSetEnabled: false, canUninstall: false, canUpdate: false },
  compatibility: { issues: [], status: 'compatible' },
  description: 'Built into the application.',
  enabled: true,
  id: 'bundled-skill-id',
  name: 'Bundled reviewer',
  packageRevision: 'package-revision-bundled',
  source: { id: 'bundled-source-id', kind: 'bundled' },
  stateRevision: 'state-revision-bundled'
}

const installedSkill: SkillManagementEntry = {
  actions: { canSetEnabled: true, canUninstall: true, canUpdate: true },
  acquisition: {
    displayName: 'repository-auditor',
    kind: 'localDirectory',
    refreshable: true
  },
  compatibility: {
    issues: [
      {
        code: 'savedScripts',
        id: 'saved-scripts',
        message: 'Scripts are stored but never executed by the application.',
        requiresAcknowledgement: false,
        severity: 'warning'
      }
    ],
    status: 'compatibleWithWarnings'
  },
  description: 'Audits repository evidence.',
  enabled: false,
  id: 'installed-skill-id',
  installationRevision: 'installation-revision-1',
  name: 'Repository auditor',
  packageRevision: 'package-revision-installed',
  source: { id: 'installed-source-id', kind: 'installed' },
  stateRevision: 'state-revision-installed'
}

const workspaceSkill: SkillManagementEntry = {
  ...installedSkill,
  actions: { canSetEnabled: false, canUninstall: false, canUpdate: false },
  id: 'workspace-skill-id',
  name: 'Workspace-only skill',
  source: { id: 'workspace-source-id', kind: 'workspace' }
}

const bundledOfficeSkills: SkillManagementEntry[] = [
  {
    ...bundledSkill,
    id: 'bundled:application:documents',
    name: 'Documents',
    source: { id: 'application:documents', kind: 'bundled' }
  },
  {
    ...bundledSkill,
    id: 'bundled:application:spreadsheets',
    name: 'Spreadsheets',
    source: { id: 'application:spreadsheets', kind: 'bundled' }
  },
  {
    ...bundledSkill,
    id: 'bundled:application:presentations',
    name: 'Presentations',
    source: { id: 'application:presentations', kind: 'bundled' }
  }
]

const bundledImageGenerationSkill: SkillManagementEntry = {
  ...bundledSkill,
  actions: { canSetEnabled: true, canUninstall: false, canUpdate: false },
  id: 'bundled:application:image-generation',
  name: 'image-generation',
  source: { id: 'application:image-generation', kind: 'bundled' }
}

function managementOutput(
  skills: SkillManagementEntry[] = [bundledSkill, installedSkill, workspaceSkill],
  overrides: Partial<SkillsListManagementOutput> = {}
): SkillsListManagementOutput {
  return {
    diagnostics: [],
    managementRevision: 'management-revision-1',
    schemaVersion: 1,
    skills,
    truncated: false,
    ...overrides
  }
}

function installationPreview(
  overrides: Partial<SkillInstallationPreview> = {}
): SkillInstallationPreview {
  return {
    changes: { content: 'new', source: 'new' },
    compatibility: {
      issues: [
        {
          code: 'scriptsPresent',
          id: 'scripts-acknowledgement',
          message: 'This package contains scripts. They will be stored but not executed.',
          requiresAcknowledgement: true,
          severity: 'warning'
        }
      ],
      status: 'compatibleWithWarnings'
    },
    expiresAtUnixMs: Date.now() + 60_000,
    installationId: 'installation-id',
    operation: 'install',
    package: {
      description: 'Audits repository evidence.',
      fileCount: 4,
      formatVersion: 2,
      name: 'Repository auditor',
      packageRevision: 'package-revision-preview',
      totalBytes: 4096
    },
    preparationId: 'preparation-id',
    previewRevision: 'preview-revision',
    schemaVersion: 1,
    skillId: 'installed-skill-id',
    source: {
      displayName: 'repository-auditor',
      kind: 'localDirectory',
      refreshable: true
    },
    ...overrides
  }
}

function sourceCandidate(
  overrides: Partial<SkillSourceResolutionCandidate> = {}
): SkillSourceResolutionCandidate {
  const resolutionId = overrides.acquisition?.resolutionId ?? '11111111-1111-4111-8111-111111111111'
  const candidateId = overrides.candidateId ?? 'candidate-reviewer'
  return {
    acquisition: { candidateId, kind: 'resolvedCandidate', resolutionId },
    candidateId,
    package: {
      description: 'Audits repository evidence.',
      fileCount: 4,
      formatVersion: 2,
      name: 'Repository auditor',
      packageRevision: 'package-revision-candidate',
      totalBytes: 4096
    },
    source: {
      kind: 'githubRepository',
      owner: 'openai',
      reference: { kind: 'defaultBranch' },
      repository: 'skills',
      resolvedCommit: '0123456789abcdef0123456789abcdef01234567',
      subdirectory: 'skills/reviewer'
    },
    ...overrides
  }
}

function resolutionOutput(
  candidates: readonly SkillSourceResolutionCandidate[] = [sourceCandidate()]
): SkillsResolveInstallationSourceOutput {
  const resolutionId = candidates[0]?.acquisition.resolutionId
  if (!resolutionId || candidates.length === 0) throw new Error('Resolution needs candidates')
  const base = {
    canonicalUrl: 'https://github.com/openai/skills',
    expiresAtUnixMs: Date.now() + 60_000,
    provider: 'github' as const,
    resolutionId,
    resolvedCommit: '0123456789abcdef0123456789abcdef01234567',
    schemaVersion: 2 as const
  }
  if (candidates.length === 1) {
    return { ...base, candidates: [candidates[0]], outcome: 'resolved' }
  }
  const [first, second, ...rest] = candidates
  if (!first || !second) throw new Error('Selection requires two candidates')
  return { ...base, candidates: [first, second, ...rest], outcome: 'selectionRequired' }
}

function commitOutput(operation: 'install' | 'update' = 'install'): SkillInstallationCommitOutput {
  return {
    changes: { content: operation === 'install' ? 'new' : 'changed', source: 'changed' },
    installationId: 'installation-id',
    installationRevision: 'installation-revision-2',
    operation,
    outcome: operation === 'install' ? 'installed' : 'updated',
    packageRevision: 'package-revision-committed',
    preparationId: 'preparation-id',
    schemaVersion: 1,
    skillId: 'installed-skill-id'
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, reject, resolve }
}

beforeEach(() => {
  changedHandler = undefined
  for (const spy of Object.values(service)) spy.mockReset()
  service.cancelPreparation.mockResolvedValue({
    outcome: 'cancelled',
    preparationId: 'preparation-id',
    schemaVersion: 1
  })
  service.cancelSourceResolution.mockImplementation(
    ({ resolutionId }: { resolutionId: string }): Promise<SkillsCancelSourceResolutionOutput> =>
      Promise.resolve({ outcome: 'cancelled', resolutionId, schemaVersion: 2 })
  )
  service.commitInstallation.mockResolvedValue(commitOutput())
  service.inspectInstallation.mockResolvedValue(installationPreview())
  service.listManagement.mockResolvedValue(managementOutput())
  service.onChanged.mockImplementation((handler: () => void) => {
    changedHandler = handler
    return service.unsubscribe
  })
  service.selectInstallationDirectory.mockResolvedValue('/tmp/repository-auditor')
  service.resolveInstallationSource.mockImplementation(
    ({ resolutionId }: { resolutionId: string }) => {
      const candidate = sourceCandidate({
        acquisition: {
          candidateId: 'candidate-reviewer',
          kind: 'resolvedCandidate',
          resolutionId
        }
      })
      return Promise.resolve(resolutionOutput([candidate]))
    }
  )
  service.setEnabled.mockResolvedValue({
    enabled: true,
    managementRevision: 'management-revision-2',
    outcome: 'updated',
    schemaVersion: 1,
    skillId: installedSkill.id,
    stateRevision: 'state-revision-2'
  })
  service.uninstall.mockResolvedValue({
    installationId: 'installation-id',
    outcome: 'uninstalled',
    schemaVersion: 1,
    skillId: installedSkill.id
  })
})

describe('Skills settings navigation and management inventory', () => {
  it('shows Skills in the Coding navigation and opens the page', async () => {
    const onBack = vi.fn()
    const screen = await render(
      <SettingsPage
        conversations={[]}
        initialPage="environment"
        onBack={onBack}
        onDeleteArchivedConversations={vi.fn()}
        onDeleteConversation={vi.fn()}
        onRemoveProject={vi.fn().mockResolvedValue(true)}
        onUiPreferencesChange={vi.fn()}
        onUnarchiveConversation={vi.fn()}
        projects={[]}
        uiPreferences={{
          customPermissionEnabled: true,
          customPermissions: {
            command: 'require_approval',
            commandSafety: 'guarded',
            patch: 'require_approval',
            read: 'workspace_only',
            write: 'workspace_only'
          },
          fullPermissionEnabled: true,
          nativeFontSmoothing: false,
          profileAvatarDataUrl: null,
          profileDisplayName: '',
          profileHandle: 'USER',
          showContextWindowUsage: true,
          showTokenUsageDetails: true,
          sidebarConversationSort: 'updated',
          sidebarProjectOrder: [],
          sidebarProjectSort: 'created',
          sidebarSectionOrder: 'projects_first',
          translucentSidebar: false,
          translucentSidebarTransparency: 54,
          updatedAt: 0
        }}
      />
    )

    const skillsNavigation = screen.getByRole('button', { name: 'settings.page.skills' })
    await expect.element(skillsNavigation).toBeVisible()
    await skillsNavigation.click()
    await expect
      .element(screen.getByRole('heading', { level: 1, name: 'settings.page.skills' }))
      .toBeVisible()

    await screen.getByRole('button', { name: 'settings.nav.mcp' }).click()
    await expect.element(screen.getByText('mcp-page')).toBeVisible()
    await screen.getByRole('button', { name: 'make-mcp-dirty' }).click()
    await screen.getByRole('button', { name: 'settings.page.environment' }).click()
    await expect.element(screen.getByRole('heading', { name: 'mcp.unsaved.title' })).toBeVisible()
    await screen.getByText('mcp.actions.cancel', { exact: true }).click()
    await expect.element(screen.getByText('mcp-page')).toBeVisible()

    await screen.getByRole('button', { name: 'settings.page.environment' }).click()
    await screen.getByRole('button', { name: 'mcp.unsaved.discard' }).click()
    await expect.element(screen.getByText('environment-page')).toBeVisible()

    await screen.getByRole('button', { name: 'settings.nav.mcp' }).click()
    await screen.getByRole('button', { name: 'make-mcp-dirty' }).click()
    await screen.getByRole('button', { name: 'settings.backToApp' }).click()
    await screen.getByRole('button', { name: 'mcp.unsaved.discard' }).click()
    expect(onBack).toHaveBeenCalledTimes(1)
  })

  it('renders loading and empty states', async () => {
    const pending = deferred<SkillsListManagementOutput>()
    service.listManagement.mockReturnValueOnce(pending.promise)
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText('skills.loading')).toBeVisible()
    pending.resolve(managementOutput([]))
    await expect.element(screen.getByText('skills.empty', { exact: true })).toBeVisible()
  })

  it('renders a load failure and retries', async () => {
    service.listManagement
      .mockRejectedValueOnce(new Error('management unavailable'))
      .mockResolvedValueOnce(managementOutput([]))
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText('management unavailable')).toBeVisible()
    await screen.getByRole('button', { name: 'skills.retry' }).click()
    await expect.element(screen.getByText('skills.empty', { exact: true })).toBeVisible()
  })

  it('shows backend-authorized actions and filters workspace skills', async () => {
    const screen = await render(
      <StrictMode>
        <SkillsSettingsPage />
      </StrictMode>
    )
    await expect.element(screen.getByText(bundledSkill.name)).toBeVisible()
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await expect.element(screen.getByText('skills.sourceBundled')).toBeVisible()
    await expect.element(screen.getByText('skills.sourceInstalled')).toBeVisible()
    expect(screen.container.textContent).not.toContain(workspaceSkill.name)

    const bundledRow = findSkillRow(screen.container, bundledSkill.name)
    const installedRow = findSkillRow(screen.container, installedSkill.name)
    expect(bundledRow.querySelectorAll('.skill-row-action')).toHaveLength(0)
    expect(bundledRow.querySelector<HTMLButtonElement>('[role="switch"]')?.disabled).toBe(true)
    expect(installedRow.querySelectorAll('.skill-row-action')).toHaveLength(2)
    expect(installedRow.textContent).toContain('Scripts are stored but never executed')
    expect(screen.container.textContent).not.toContain(installedSkill.description)
  })

  it('uses dedicated Office icons for bundled document, spreadsheet, and presentation skills', async () => {
    service.listManagement.mockResolvedValueOnce(managementOutput(bundledOfficeSkills))
    const screen = await render(<SkillsSettingsPage />)

    for (const [name, kind] of [
      ['skills.bundled.documents.name', 'document'],
      ['skills.bundled.spreadsheets.name', 'spreadsheet'],
      ['skills.bundled.presentations.name', 'presentation']
    ] as const) {
      await expect.element(screen.getByText(name)).toBeVisible()
      const row = findSkillRow(screen.container, name)
      const icon = row.querySelector(`[data-office-kind="${kind}"]`)
      expect(icon?.querySelector('img')).not.toBeNull()
      expect(icon?.querySelector('svg')).toBeNull()
    }
  })

  it('uses the localized Image Generation presentation and its non-Office icon', async () => {
    service.listManagement.mockResolvedValueOnce(managementOutput([bundledImageGenerationSkill]))
    const screen = await render(<SkillsSettingsPage />)

    const name = 'skills.bundled.imageGeneration.name'
    await expect.element(screen.getByText(name)).toBeVisible()
    const row = findSkillRow(screen.container, name)
    const icon = row.querySelector('.skill-management-row__icon')
    expect(icon?.querySelector('svg')).not.toBeNull()
    expect(icon?.querySelector('img')).toBeNull()
    expect(row.querySelector<HTMLButtonElement>('[role="switch"]')?.disabled).toBe(false)
  })

  it('does not offer update when backend actions deny it for a local installation', async () => {
    const localOnly = {
      ...installedSkill,
      actions: { ...installedSkill.actions, canUpdate: false },
      id: 'local-only-skill',
      name: 'Local-only skill'
    }
    service.listManagement.mockResolvedValueOnce(managementOutput([localOnly]))
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(localOnly.name)).toBeVisible()
    const row = findSkillRow(screen.container, localOnly.name)
    expect(row.querySelector('[aria-label="skills.updateNamed"]')).toBeNull()
    expect(row.querySelectorAll('.skill-row-action')).toHaveLength(1)
  })
})

describe('Skills management mutations', () => {
  it('uses the exact state revision and disables the row switch while pending', async () => {
    const pending = deferred<{
      enabled: boolean
      managementRevision: string
      outcome: 'updated'
      schemaVersion: 1
      skillId: string
      stateRevision: string
    }>()
    service.setEnabled.mockReturnValueOnce(pending.promise)
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    const row = findSkillRow(screen.container, installedSkill.name)
    const toggle = row.querySelector<HTMLButtonElement>('[role="switch"]')
    if (!toggle) throw new Error('Missing installed Skill switch')

    toggle.click()
    expect(service.setEnabled).toHaveBeenCalledWith({
      enabled: true,
      expectedStateRevision: installedSkill.stateRevision,
      skillId: installedSkill.id
    })
    await expect.poll(() => toggle.disabled).toBe(true)
    pending.resolve({
      enabled: true,
      managementRevision: 'management-revision-2',
      outcome: 'updated',
      schemaVersion: 1,
      skillId: installedSkill.id,
      stateRevision: 'state-revision-2'
    })
    await expect.poll(() => toggle.getAttribute('aria-checked')).toBe('true')
    expect(service.showToast).not.toHaveBeenCalled()
  })

  it('refreshes after a structured state conflict', async () => {
    service.setEnabled.mockRejectedValueOnce(
      new HostInvocationError({
        data: {
          code: 'stateConflict',
          message: 'State changed',
          operation: 'setEnabled',
          recovery: 'refreshManagement',
          type: 'skillManagement'
        },
        message: 'State changed'
      })
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    const toggle = findSkillRow(
      screen.container,
      installedSkill.name
    ).querySelector<HTMLButtonElement>('[role="switch"]')
    if (!toggle) throw new Error('Missing installed Skill switch')
    toggle.click()
    await expect.poll(() => service.listManagement.mock.calls.length).toBeGreaterThanOrEqual(2)
    await expect
      .poll(() => service.showToast.mock.calls)
      .toContainEqual(['skills.stateChanged', { durationMs: 3200 }])
  })

  it('refreshes on skills.changed and unsubscribes on unmount', async () => {
    const screen = await render(<SkillsUnmountHarness />)
    await expect.poll(() => service.listManagement.mock.calls.length).toBe(1)
    changedHandler?.()
    await expect.poll(() => service.listManagement.mock.calls.length).toBe(2)
    await screen.getByRole('button', { name: 'unmount-skills-page' }).click()
    expect(service.unsubscribe).toHaveBeenCalledOnce()
  })

  it('confirms uninstall with installationRevision rather than stateRevision', async () => {
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    findSkillRow(screen.container, installedSkill.name)
      .querySelector<HTMLButtonElement>('.skill-row-action--danger')
      ?.click()
    await screen.getByRole('button', { name: 'skills.confirmUninstall' }).click()
    await expect.poll(() => service.uninstall.mock.calls.length).toBe(1)
    expect(service.uninstall).toHaveBeenCalledWith({
      expectedRevision: installedSkill.installationRevision,
      skillId: installedSkill.id
    })
  })
})

describe('Skill installation and update workflow', () => {
  it('moves focus into the installation dialog and restores it when closed', async () => {
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    const installButton = screen.getByRole('button', { exact: true, name: 'skills.install' })
    const installButtonElement = installButton.element()
    installButtonElement.focus()
    await installButton.click()

    const closeButton = document.querySelector<HTMLButtonElement>(
      '[aria-label="skills.closeDialog"]'
    )
    if (!closeButton) throw new Error('Missing installation dialog close button')
    await expect.poll(() => document.activeElement).toBe(closeButton)

    closeButton.click()
    await expect.poll(() => document.activeElement).toBe(installButtonElement)
  })

  it('treats a cancelled local directory picker as cancellation without inspection', async () => {
    service.selectInstallationDirectory.mockResolvedValueOnce(null)
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    expect(service.inspectInstallation).not.toHaveBeenCalled()
  })

  it('chooses an installation source before showing the GitHub URL form', async () => {
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()

    await expect
      .element(screen.getByRole('button', { name: /skills.installFromGitHub/ }))
      .toBeVisible()
    await expect.element(screen.getByRole('button', { name: /skills.installLocal/ })).toBeVisible()
    expect(document.querySelector('input[type="url"]')).toBeNull()

    await screen.getByRole('button', { name: /skills.installFromGitHub/ }).click()
    await expect.element(screen.getByRole('textbox', { name: 'skills.githubUrl' })).toBeVisible()
    await screen.getByRole('button', { name: 'skills.back' }).click()
    await expect
      .element(screen.getByRole('button', { name: /skills.installFromGitHub/ }))
      .toBeVisible()
  })

  it('completes local inspect, acknowledgement, frozen preview commit, and refresh', async () => {
    const screen = await render(
      <StrictMode>
        <SkillsSettingsPage />
      </StrictMode>
    )
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await expect
      .element(screen.getByRole('heading', { level: 3, name: 'Repository auditor' }))
      .toBeVisible()

    const commitButton = screen.getByRole('button', { name: 'skills.installOperation' })
    await expect.element(commitButton).toBeDisabled()
    await screen.getByRole('checkbox', { name: /This package contains scripts/ }).click()
    await expect.element(commitButton).toBeEnabled()
    await commitButton.click()

    expect(service.inspectInstallation).toHaveBeenCalledWith({
      intent: { operation: 'install' },
      preparationId: expect.any(String),
      source: { directory: '/tmp/repository-auditor', kind: 'localDirectory' }
    })
    expect(service.commitInstallation).toHaveBeenCalledWith({
      acceptedIssueIds: ['scripts-acknowledgement'],
      preparationId: 'preparation-id',
      previewRevision: 'preview-revision'
    })
    await expect.poll(() => service.listManagement.mock.calls.length).toBeGreaterThanOrEqual(2)
    expect(service.showToast).not.toHaveBeenCalled()
  })

  it('updates directly through installedSource with expectedInstallationRevision', async () => {
    service.inspectInstallation.mockResolvedValueOnce(installationPreview({ operation: 'update' }))
    service.commitInstallation.mockResolvedValueOnce(commitOutput('update'))
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    const row = findSkillRow(screen.container, installedSkill.name)
    row
      .querySelector<HTMLButtonElement>('.skill-row-action:not(.skill-row-action--danger)')
      ?.click()
    await expect.poll(() => service.inspectInstallation.mock.calls.length).toBe(1)
    await expect.poll(() => row.getAttribute('aria-busy')).toBe('true')
    expect(service.inspectInstallation).toHaveBeenCalledWith({
      intent: {
        expectedInstallationRevision: installedSkill.installationRevision,
        operation: 'update',
        skillId: installedSkill.id
      },
      preparationId: expect.any(String),
      source: { kind: 'installedSource' }
    })
  })

  it('presents a concise preview and keeps protocol metadata in technical details', async () => {
    service.inspectInstallation.mockResolvedValueOnce(
      installationPreview({
        compatibility: { issues: [], status: 'compatible' },
        source: {
          kind: 'githubRepository',
          owner: 'openai',
          reference: { kind: 'named', value: 'main' },
          refreshable: true,
          repository: 'skills',
          resolvedCommit: '0123456789abcdef0123456789abcdef01234567',
          subdirectory: 'skills/reviewer'
        }
      })
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()

    await expect.element(screen.getByText('openai/skills')).toBeVisible()
    await expect.element(screen.getByText('skills.previewContents')).toBeVisible()
    await expect
      .element(screen.getByText('skills.previewReadyToInstall', { exact: true }))
      .toBeVisible()
    await expect
      .element(screen.getByText('skills.previewSafetyPassed', { exact: true }))
      .toBeVisible()
    expect(document.body.textContent).not.toContain('skills.previewExactSource')

    await screen.getByRole('button', { name: 'skills.showTechnicalDetails' }).click()
    await expect.element(screen.getByText('skills.previewExactSource')).toBeVisible()
    await expect.element(screen.getByText('openai/skills · main · skills/reviewer')).toBeVisible()
    await expect.element(screen.getByText('skills.previewFormat')).toBeVisible()
    await expect.element(screen.getByText('skills.previewExpires')).toBeVisible()
  })

  it('keeps preview identity and actions fixed while technical details scroll independently', async () => {
    service.inspectInstallation.mockResolvedValueOnce(
      installationPreview({
        compatibility: { issues: [], status: 'compatible' },
        package: {
          description: 'A '.repeat(900),
          fileCount: 4,
          formatVersion: 3,
          name: 'Long preview skill',
          packageRevision: 'package-revision-long-preview',
          totalBytes: 28_600
        }
      })
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await screen.getByRole('button', { name: 'skills.showTechnicalDetails' }).click()

    const dialog = document.querySelector<HTMLElement>('.skill-install-dialog')
    const body = document.querySelector<HTMLElement>('.skill-install-dialog__body')
    const identity = document.querySelector<HTMLElement>('.skill-install-preview__identity')
    const scrollRegion = document.querySelector<HTMLElement>(
      '.skill-install-preview__scroll-region'
    )
    const actions = document.querySelector<HTMLElement>('.skill-install-preview__actions')
    const technicalFacts = document.querySelector<HTMLElement>(
      '.skill-install-preview__technical-facts'
    )

    expect(dialog).not.toBeNull()
    expect(body).not.toBeNull()
    expect(identity).not.toBeNull()
    expect(scrollRegion).not.toBeNull()
    expect(actions).not.toBeNull()
    expect(technicalFacts).not.toBeNull()
    expect(scrollRegion?.contains(technicalFacts)).toBe(true)
    expect(scrollRegion?.contains(identity)).toBe(false)
    expect(scrollRegion?.contains(actions)).toBe(false)
    expect(getComputedStyle(body!).overflowY).toBe('hidden')
    expect(getComputedStyle(scrollRegion!).overflowY).toBe('auto')
    expect(scrollRegion!.scrollHeight).toBeGreaterThan(scrollRegion!.clientHeight)
    await expect.element(screen.getByRole('button', { name: 'skills.back' })).toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: 'skills.installOperation' }))
      .toBeVisible()
  })

  it('treats an unchanged update as already current and does not offer a commit action', async () => {
    service.inspectInstallation.mockResolvedValueOnce(
      installationPreview({
        changes: { content: 'unchanged', source: 'unchanged' },
        compatibility: { issues: [], status: 'compatible' },
        operation: 'update'
      })
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    const row = findSkillRow(screen.container, installedSkill.name)
    row
      .querySelector<HTMLButtonElement>('.skill-row-action:not(.skill-row-action--danger)')
      ?.click()

    await expect
      .element(screen.getByText('skills.previewAlreadyCurrent', { exact: true }))
      .toBeVisible()
    await expect.element(screen.getByRole('button', { name: 'skills.done' })).toBeVisible()
    expect(
      Array.from(document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')).some(
        (button) => button.textContent === 'skills.updateOperation'
      )
    ).toBe(false)
    expect(service.commitInstallation).not.toHaveBeenCalled()
  })

  it('resolves a single URL candidate and passes its acquisition authority unchanged', async () => {
    const exactAcquisition = {
      candidateId: 'opaque-candidate-token',
      kind: 'resolvedCandidate' as const,
      resolutionId: '22222222-2222-4222-8222-222222222222'
    }
    service.resolveInstallationSource.mockResolvedValueOnce(
      resolutionOutput([sourceCandidate({ acquisition: exactAcquisition })])
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installFromGitHub/ }).click()
    await screen
      .getByRole('textbox', { name: 'skills.githubUrl' })
      .fill('https://github.com/openai/skills/blob/main/skills/reviewer/SKILL.md')
    await screen.getByRole('button', { name: 'skills.continue' }).click()
    await expect.poll(() => service.resolveInstallationSource.mock.calls.length).toBe(1)
    await expect.poll(() => service.inspectInstallation.mock.calls.length).toBe(1)
    expect(service.resolveInstallationSource).toHaveBeenCalledWith({
      locator: {
        kind: 'url',
        url: 'https://github.com/openai/skills/blob/main/skills/reviewer/SKILL.md'
      },
      resolutionId: expect.any(String)
    })
    expect(service.inspectInstallation).toHaveBeenCalledWith({
      intent: { operation: 'install' },
      preparationId: expect.any(String),
      source: exactAcquisition
    })
  })

  it('renders multiple candidates and inspects the selected opaque acquisition', async () => {
    const resolutionId = '33333333-3333-4333-8333-333333333333'
    const firstAcquisition = {
      candidateId: 'candidate-first',
      kind: 'resolvedCandidate' as const,
      resolutionId
    }
    const secondAcquisition = {
      candidateId: 'candidate-second',
      kind: 'resolvedCandidate' as const,
      resolutionId
    }
    service.resolveInstallationSource.mockResolvedValueOnce(
      resolutionOutput([
        sourceCandidate({ acquisition: firstAcquisition, candidateId: 'candidate-first' }),
        sourceCandidate({
          acquisition: secondAcquisition,
          candidateId: 'candidate-second',
          package: {
            description: 'Checks release readiness.',
            fileCount: 7,
            formatVersion: 2,
            name: 'Release checker',
            packageRevision: 'package-release',
            totalBytes: 8192
          },
          source: {
            kind: 'githubRepository',
            owner: 'openai',
            reference: { kind: 'named', value: 'main' },
            repository: 'skills',
            resolvedCommit: '0123456789abcdef0123456789abcdef01234567',
            subdirectory: 'skills/release'
          }
        })
      ])
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installFromGitHub/ }).click()
    await screen
      .getByRole('textbox', { name: 'skills.githubUrl' })
      .fill('https://github.com/openai/skills')
    await screen.getByRole('button', { name: 'skills.continue' }).click()

    await expect.element(screen.getByText('Release checker')).toBeVisible()
    await screen.getByRole('button', { name: 'skills.chooseCandidateNamed' }).nth(1).click()
    await expect.poll(() => service.inspectInstallation.mock.calls.length).toBe(1)
    expect(service.inspectInstallation.mock.calls[0]?.[0].source).toEqual(secondAcquisition)
  })

  it('cancels an abandoned resolution and fences a late success from reopening the dialog', async () => {
    const pending = deferred<SkillsResolveInstallationSourceOutput>()
    service.resolveInstallationSource.mockReturnValueOnce(pending.promise)
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installFromGitHub/ }).click()
    await screen
      .getByRole('textbox', { name: 'skills.githubUrl' })
      .fill('https://github.com/openai/skills')
    await screen.getByRole('button', { name: 'skills.continue' }).click()
    await expect.element(screen.getByText('skills.resolvingSource')).toBeVisible()

    const resolutionId = service.resolveInstallationSource.mock.calls[0]?.[0].resolutionId
    document.querySelector<HTMLButtonElement>('[aria-label="skills.closeDialog"]')?.click()
    await expect.poll(() => service.cancelSourceResolution.mock.calls.length).toBe(1)
    expect(service.cancelSourceResolution).toHaveBeenCalledWith({ resolutionId })

    pending.resolve(
      resolutionOutput([
        sourceCandidate({
          acquisition: { candidateId: 'late-candidate', kind: 'resolvedCandidate', resolutionId }
        })
      ])
    )
    await expect
      .poll(() => service.cancelSourceResolution.mock.calls.length)
      .toBeGreaterThanOrEqual(2)
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    expect(service.inspectInstallation).not.toHaveBeenCalled()
  })

  it('cancels candidate authority when returning to edit the URL', async () => {
    const resolutionId = '44444444-4444-4444-8444-444444444444'
    service.resolveInstallationSource.mockResolvedValueOnce(
      resolutionOutput([
        sourceCandidate({
          acquisition: { candidateId: 'first', kind: 'resolvedCandidate', resolutionId },
          candidateId: 'first'
        }),
        sourceCandidate({
          acquisition: { candidateId: 'second', kind: 'resolvedCandidate', resolutionId },
          candidateId: 'second',
          package: {
            description: 'Second skill.',
            fileCount: 2,
            formatVersion: 1,
            name: 'Second skill',
            packageRevision: 'second-package',
            totalBytes: 1024
          }
        })
      ])
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installFromGitHub/ }).click()
    const input = screen.getByRole('textbox', { name: 'skills.githubUrl' })
    await input.fill('https://github.com/openai/skills')
    await screen.getByRole('button', { name: 'skills.continue' }).click()
    await expect.element(screen.getByText('skills.chooseCandidateTitle')).toBeVisible()
    await screen.getByRole('button', { name: 'skills.back' }).click()
    await expect.element(screen.getByRole('textbox', { name: 'skills.githubUrl' })).toBeVisible()
    expect(service.cancelSourceResolution).toHaveBeenCalledWith({
      resolutionId: service.resolveInstallationSource.mock.calls[0]?.[0].resolutionId
    })
  })

  it('cancels a late inspection preview instead of rendering stale state', async () => {
    const pending = deferred<SkillInstallationPreview>()
    service.inspectInstallation.mockReturnValueOnce(pending.promise)
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await expect.element(screen.getByText('skills.inspecting')).toBeVisible()
    const preparationId = service.inspectInstallation.mock.calls[0]?.[0].preparationId

    document.querySelector<HTMLButtonElement>('[aria-label="skills.closeDialog"]')?.click()
    pending.resolve(installationPreview({ preparationId }))
    await expect.poll(() => service.cancelPreparation.mock.calls.length).toBeGreaterThanOrEqual(2)
    expect(service.cancelPreparation).toHaveBeenCalledWith({ preparationId })
    expect(document.querySelector('[role="dialog"]')).toBeNull()
  })

  it('never renders the selected local directory path', async () => {
    const localPath = '/private/secret/user-skill'
    service.selectInstallationDirectory.mockResolvedValueOnce(localPath)
    service.inspectInstallation.mockResolvedValueOnce(
      installationPreview({
        compatibility: { issues: [], status: 'compatible' },
        source: { displayName: localPath, kind: 'localDirectory', refreshable: false }
      })
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await expect.element(screen.getByText('skills.previewLocalSource')).toBeVisible()
    expect(document.body.textContent).not.toContain(localPath)
  })

  it('shows resourcesNotExposed without requiring acknowledgement', async () => {
    service.inspectInstallation.mockResolvedValueOnce(
      installationPreview({
        compatibility: {
          issues: [
            {
              code: 'resourcesNotExposed',
              id: 'resources-warning',
              message: 'Some resources are stored but not exposed.',
              requiresAcknowledgement: false,
              severity: 'warning'
            }
          ],
          status: 'compatibleWithWarnings'
        }
      })
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await expect
      .element(screen.getByText('Some resources are stored but not exposed.'))
      .toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: 'skills.installOperation' }))
      .toBeEnabled()
    expect(document.querySelector('[role="dialog"] input[type="checkbox"]')).toBeNull()
  })

  it('prevents closing the dialog while commit is pending', async () => {
    const pending = deferred<SkillInstallationCommitOutput>()
    service.commitInstallation.mockReturnValueOnce(pending.promise)
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await screen.getByRole('checkbox', { name: /This package contains scripts/ }).click()
    await screen.getByRole('button', { name: 'skills.installOperation' }).click()

    const closeButton = document.querySelector<HTMLButtonElement>(
      '[aria-label="skills.closeDialog"]'
    )
    expect(closeButton?.disabled).toBe(true)
    document.querySelector<HTMLElement>('.skill-install-dialog__backdrop')?.click()
    expect(document.querySelector('[role="dialog"]')).not.toBeNull()
    pending.resolve(commitOutput())
    await expect.poll(() => document.querySelector('[role="dialog"]')).toBeNull()
  })

  it('retries a recoverable inspection with the same preparation id', async () => {
    service.inspectInstallation
      .mockRejectedValueOnce(
        new HostInvocationError({
          data: {
            code: 'networkUnavailable',
            message: 'GitHub is temporarily unavailable',
            phase: 'inspect',
            preparationId: 'server-preparation-id',
            recovery: 'retrySamePreparation',
            type: 'skillInspection'
          },
          message: 'GitHub is temporarily unavailable'
        })
      )
      .mockResolvedValueOnce(installationPreview())
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await expect.element(screen.getByText('skills.localOperationFailed')).toBeVisible()

    const firstPreparationId = service.inspectInstallation.mock.calls[0]?.[0].preparationId
    await screen.getByRole('button', { name: 'skills.retry' }).click()
    await expect.poll(() => service.inspectInstallation.mock.calls.length).toBe(2)
    expect(service.inspectInstallation.mock.calls[1]?.[0].preparationId).toBe(firstPreparationId)
  })

  it('retries source resolution with the same frozen URL and resolution id', async () => {
    service.resolveInstallationSource
      .mockRejectedValueOnce(
        new HostInvocationError({
          data: {
            code: 'networkUnavailable',
            message: 'GitHub is temporarily unavailable',
            phase: 'resolve',
            recovery: 'retrySameResolution',
            type: 'skillSourceResolution'
          },
          message: 'GitHub is temporarily unavailable'
        })
      )
      .mockImplementationOnce(({ resolutionId }: { resolutionId: string }) =>
        Promise.resolve(
          resolutionOutput([
            sourceCandidate({
              acquisition: {
                candidateId: 'retried-candidate',
                kind: 'resolvedCandidate',
                resolutionId
              }
            })
          ])
        )
      )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installFromGitHub/ }).click()
    await screen
      .getByRole('textbox', { name: 'skills.githubUrl' })
      .fill('https://github.com/openai/skills')
    await screen.getByRole('button', { name: 'skills.continue' }).click()
    await expect.element(screen.getByText('GitHub is temporarily unavailable')).toBeVisible()
    const firstInput = service.resolveInstallationSource.mock.calls[0]?.[0]
    await screen.getByRole('button', { name: 'skills.retry' }).click()
    await expect.poll(() => service.resolveInstallationSource.mock.calls.length).toBe(2)
    expect(service.resolveInstallationSource.mock.calls[1]?.[0]).toEqual(firstInput)
  })

  it('resolveAgain creates new resolution and preparation identities', async () => {
    service.inspectInstallation
      .mockRejectedValueOnce(
        new HostInvocationError({
          data: {
            code: 'resolutionConsumed',
            message: 'Candidate authority was consumed',
            phase: 'inspect',
            recovery: 'resolveAgain',
            type: 'skillInspection'
          },
          message: 'Candidate authority was consumed'
        })
      )
      .mockResolvedValueOnce(installationPreview())
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installFromGitHub/ }).click()
    await screen
      .getByRole('textbox', { name: 'skills.githubUrl' })
      .fill('https://github.com/openai/skills')
    await screen.getByRole('button', { name: 'skills.continue' }).click()
    await expect.element(screen.getByText('Candidate authority was consumed')).toBeVisible()
    const firstResolutionId = service.resolveInstallationSource.mock.calls[0]?.[0].resolutionId
    const firstPreparationId = service.inspectInstallation.mock.calls[0]?.[0].preparationId

    await screen.getByRole('button', { name: 'skills.inspectAgain' }).click()
    await expect.poll(() => service.resolveInstallationSource.mock.calls.length).toBe(2)
    await expect.poll(() => service.inspectInstallation.mock.calls.length).toBe(2)
    expect(service.resolveInstallationSource.mock.calls[1]?.[0].resolutionId).not.toBe(
      firstResolutionId
    )
    expect(service.inspectInstallation.mock.calls[1]?.[0].preparationId).not.toBe(
      firstPreparationId
    )
  })

  it('retries a commit-phase retrySamePreparation by replaying commit, not inspect', async () => {
    service.commitInstallation
      .mockRejectedValueOnce(
        new HostInvocationError({
          data: {
            code: 'networkUnavailable',
            message: 'Commit response was unavailable',
            phase: 'commit',
            preparationId: 'preparation-id',
            recovery: 'retrySamePreparation',
            type: 'skillInspection'
          },
          message: 'Commit response was unavailable'
        })
      )
      .mockResolvedValueOnce(commitOutput())
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await screen.getByRole('checkbox', { name: /This package contains scripts/ }).click()
    await screen.getByRole('button', { name: 'skills.installOperation' }).click()
    await expect.element(screen.getByText('skills.localOperationFailed')).toBeVisible()
    const firstCommitInput = service.commitInstallation.mock.calls[0]?.[0]

    await screen.getByRole('button', { name: 'skills.retry' }).click()
    await expect.poll(() => service.commitInstallation.mock.calls.length).toBe(2)
    expect(service.commitInstallation.mock.calls[1]?.[0]).toEqual(firstCommitInput)
    expect(service.inspectInstallation).toHaveBeenCalledTimes(1)
  })

  it('refreshes authoritative inventory after commitIndeterminate without replaying commit', async () => {
    service.listManagement
      .mockResolvedValueOnce(managementOutput())
      .mockResolvedValueOnce(
        managementOutput([
          bundledSkill,
          { ...installedSkill, installationRevision: 'installation-revision-2' }
        ])
      )
    service.commitInstallation.mockRejectedValueOnce(
      new HostInvocationError({
        data: {
          code: 'commitIndeterminate',
          commitMayHaveSucceeded: true,
          intendedInstallationRevision: 'installation-revision-2',
          message: 'Commit result is indeterminate',
          phase: 'commit',
          preparationId: 'preparation-id',
          recovery: 'refreshManagement',
          skillId: installedSkill.id,
          type: 'skillInspection'
        },
        message: 'Commit result is indeterminate'
      })
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await screen.getByRole('checkbox', { name: /This package contains scripts/ }).click()
    await screen.getByRole('button', { name: 'skills.installOperation' }).click()

    await expect.poll(() => service.listManagement.mock.calls.length).toBeGreaterThanOrEqual(2)
    expect(service.commitInstallation).toHaveBeenCalledTimes(1)
    expect(document.querySelector('[role="dialog"]')).toBeNull()
    await expect
      .poll(() => service.showToast.mock.calls)
      .toContainEqual(['skills.commitConfirmedByInventory', { durationMs: 3200 }])
  })

  it('does not allow an expired frozen preview to be committed', async () => {
    service.inspectInstallation.mockResolvedValueOnce(
      installationPreview({ expiresAtUnixMs: Date.now() - 1 })
    )
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await screen.getByRole('checkbox', { name: /This package contains scripts/ }).click()
    await expect
      .element(screen.getByRole('button', { name: 'skills.installOperation' }))
      .toBeDisabled()
    expect(service.commitInstallation).not.toHaveBeenCalled()
  })

  it('never applies an older list response over a newer invalidation refresh', async () => {
    const firstRequest = deferred<SkillsListManagementOutput>()
    service.listManagement
      .mockReturnValueOnce(firstRequest.promise)
      .mockResolvedValueOnce(managementOutput([{ ...installedSkill, name: 'Current inventory' }]))
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText('skills.loading')).toBeVisible()
    changedHandler?.()
    firstRequest.resolve(managementOutput([{ ...installedSkill, name: 'Stale inventory' }]))

    await expect.element(screen.getByText('Current inventory')).toBeVisible()
    expect(screen.container.textContent).not.toContain('Stale inventory')
  })
})

function findSkillRow(container: HTMLElement, skillName: string): HTMLElement {
  const row = Array.from(container.querySelectorAll<HTMLElement>('.skill-management-row')).find(
    (candidate) => candidate.textContent?.includes(skillName)
  )
  if (!row) throw new Error(`Missing Skill row: ${skillName}`)
  return row
}

function SkillsUnmountHarness() {
  const [mounted, setMounted] = useState(true)
  return (
    <div>
      <button type="button" onClick={() => setMounted(false)}>
        unmount-skills-page
      </button>
      {mounted && <SkillsSettingsPage />}
    </div>
  )
}
