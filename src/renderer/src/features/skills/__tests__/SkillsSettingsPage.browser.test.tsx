import { HostInvocationError } from '@mycopilot/host-api'
import type {
  SkillInstallationCommitOutput,
  SkillInstallationPreview,
  SkillManagementEntry,
  SkillsListManagementOutput
} from '@mycopilot/protocol'
import { StrictMode, useState } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

const service = vi.hoisted(() => ({
  cancelPreparation: vi.fn(),
  commitInstallation: vi.fn(),
  inspectInstallation: vi.fn(),
  listManagement: vi.fn(),
  onChanged: vi.fn(),
  selectInstallationDirectory: vi.fn(),
  setEnabled: vi.fn(),
  showToast: vi.fn(),
  uninstall: vi.fn(),
  unsubscribe: vi.fn()
}))

let changedHandler: (() => void) | undefined

vi.mock('../management/skillsManagementClient', () => ({
  cancelSkillPreparation: service.cancelPreparation,
  commitSkillInstallation: service.commitInstallation,
  inspectSkillInstallation: service.inspectInstallation,
  listManagedSkills: service.listManagement,
  onManagedSkillsChanged: service.onChanged,
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
  service.commitInstallation.mockResolvedValue(commitOutput())
  service.inspectInstallation.mockResolvedValue(installationPreview())
  service.listManagement.mockResolvedValue(managementOutput())
  service.onChanged.mockImplementation((handler: () => void) => {
    changedHandler = handler
    return service.unsubscribe
  })
  service.selectInstallationDirectory.mockResolvedValue('/tmp/repository-auditor')
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
    const screen = await render(
      <SettingsPage
        conversations={[]}
        initialPage="environment"
        onBack={vi.fn()}
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

    const commitButton = screen.getByRole('button', { name: 'skills.confirmInstallation' })
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

  it('updates through a new source and sends expectedInstallationRevision', async () => {
    service.inspectInstallation.mockResolvedValueOnce(installationPreview({ operation: 'update' }))
    service.commitInstallation.mockResolvedValueOnce(commitOutput('update'))
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    const row = findSkillRow(screen.container, installedSkill.name)
    row
      .querySelector<HTMLButtonElement>('.skill-row-action:not(.skill-row-action--danger)')
      ?.click()
    await screen.getByRole('button', { name: /skills.installLocal/ }).click()
    await expect.poll(() => service.inspectInstallation.mock.calls.length).toBe(1)
    expect(service.inspectInstallation).toHaveBeenCalledWith({
      intent: {
        expectedInstallationRevision: installedSkill.installationRevision,
        operation: 'update',
        skillId: installedSkill.id
      },
      preparationId: expect.any(String),
      source: { directory: '/tmp/repository-auditor', kind: 'localDirectory' }
    })
  })

  it('maps the GitHub form to a structured acquisition source', async () => {
    const screen = await render(<SkillsSettingsPage />)
    await expect.element(screen.getByText(installedSkill.name)).toBeVisible()
    await screen.getByRole('button', { name: 'skills.install' }).click()
    await screen.getByRole('button', { name: /skills.installGitHub/ }).click()
    await screen
      .getByRole('textbox', { name: 'skills.githubRepository' })
      .fill('https://github.com/openai/codex')
    await screen.getByRole('button', { name: /skills.githubReferenceType/ }).click()
    await screen.getByRole('option', { name: 'skills.githubNamedReference' }).click()
    await screen.getByRole('textbox', { name: 'skills.githubReferenceValue' }).fill('release/v1')
    await screen.getByRole('textbox', { name: 'skills.githubSubdirectory' }).fill('skills/reviewer')
    await screen.getByRole('button', { name: 'skills.inspect' }).click()
    await expect.poll(() => service.inspectInstallation.mock.calls.length).toBe(1)
    expect(service.inspectInstallation).toHaveBeenCalledWith({
      intent: { operation: 'install' },
      preparationId: expect.any(String),
      source: {
        kind: 'githubRepository',
        owner: 'openai',
        reference: { kind: 'named', value: 'release/v1' },
        repository: 'codex',
        subdirectory: 'skills/reviewer'
      }
    })
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
    await expect.element(screen.getByText('GitHub is temporarily unavailable')).toBeVisible()

    const firstPreparationId = service.inspectInstallation.mock.calls[0]?.[0].preparationId
    await screen.getByRole('button', { name: 'skills.retryInspection' }).click()
    await expect.poll(() => service.inspectInstallation.mock.calls.length).toBe(2)
    expect(service.inspectInstallation.mock.calls[1]?.[0].preparationId).toBe(firstPreparationId)
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
      .element(screen.getByRole('button', { name: 'skills.confirmInstallation' }))
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
