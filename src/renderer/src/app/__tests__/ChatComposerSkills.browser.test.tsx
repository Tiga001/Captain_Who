import { useState } from 'react'
import type { ComponentProps } from 'react'
import type { SkillDescriptor, SkillsListOutput } from '@mycopilot/protocol'
import { userEvent } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import '../../styles/global.css'
import '../../features/chat/components/ChatComposer.css'

const { draftChangeSpy, listSkillsSpy, submitSpy } = vi.hoisted(() => ({
  draftChangeSpy: vi.fn(),
  listSkillsSpy: vi.fn(),
  submitSpy: vi.fn()
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [
      {
        id: 'model-1',
        displayName: 'Model One',
        supportsImage: true,
        inputPrice: '0',
        outputPrice: '0',
        enabled: true
      }
    ]
  })
}))

vi.mock('../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({
    projects: [
      { id: 'project-a', name: 'Project A', path: '/workspace/a', createdAt: 1 },
      { id: 'project-b', name: 'Project B', path: '/workspace/b', createdAt: 2 }
    ],
    selectProjectDirectory: vi.fn()
  })
}))

vi.mock('../../features/chat/components/ImagePreview', () => ({
  useImagePreview: () => vi.fn()
}))

vi.mock('../../features/chat/chatAttachments', () => ({
  buildAgentInputAttachments: (attachments: unknown[]) => attachments,
  composerAttachmentFromAgentAttachment: (attachment: unknown) => attachment,
  createComposerAttachmentsFromFiles: async () => [],
  createAttachmentSummary: () => '',
  selectComposerAttachments: async () => []
}))

vi.mock('../../features/skills/skillsClient', () => ({
  listSkills: listSkillsSpy
}))

const [
  { ChatComposer },
  { ComposerSkillPicker },
  { createComposerDraft },
  skillCatalog,
  skillSelection
] = await Promise.all([
  import('../../features/chat/components/ChatComposer'),
  import('../../features/chat/components/ComposerSkillPicker'),
  import('../chatMessageFactory'),
  import('../../features/skills/skillCatalog'),
  import('../../features/skills/skillSelection')
])

const auditorSkill: SkillDescriptor = {
  activationScope: 'run',
  description: 'Collects repository evidence before making claims.',
  id: 'workspace:project-a:repository-auditor',
  location: '.agents/skills/repository-auditor/SKILL.md',
  name: 'Repository auditor',
  revision: 'skill-sha256-v1:auditor',
  source: { id: 'project-a', kind: 'workspace' },
  trust: 'untrusted'
}

const testSkill: SkillDescriptor = {
  ...auditorSkill,
  description: 'Runs focused tests for the current change.',
  id: 'workspace:project-a:test-runner',
  location: '.agents/skills/test-runner/SKILL.md',
  name: 'Test runner',
  revision: 'skill-sha256-v1:tests'
}

const projectBSkill: SkillDescriptor = {
  ...auditorSkill,
  id: 'workspace:project-b:dependency-auditor',
  location: '.agents/skills/dependency-auditor/SKILL.md',
  name: 'Dependency auditor',
  revision: 'skill-sha256-v1:dependencies',
  source: { id: 'project-b', kind: 'workspace' }
}

const bundledAuditorSkill: SkillDescriptor = {
  activationScope: 'run',
  description: 'Audits repository claims against source evidence.',
  id: 'bundled:application:repository-evidence-auditor',
  location: 'skills/repository-evidence-auditor/SKILL.md',
  name: 'Repository evidence auditor',
  revision: 'skill-sha256-v1:bundled-auditor',
  source: { id: 'bundled:application', kind: 'bundled' },
  trust: 'application'
}

const installedAuditorSkill: SkillDescriptor = {
  activationScope: 'run',
  description: 'Audits installed dependencies against repository evidence.',
  id: 'installed:user:018f7f31-7a6d-7a21-9e51-ff4b6fa4e38d',
  location: 'packages/v1/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/SKILL.md',
  name: 'Installed dependency auditor',
  revision:
    'skill-package-sha256-v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  source: { id: 'installed:user', kind: 'installed' },
  trust: 'untrusted'
}

function catalog(
  skills: SkillDescriptor[] = [auditorSkill, testSkill],
  overrides: Partial<SkillsListOutput> = {}
): SkillsListOutput {
  return {
    schemaVersion: 4,
    catalogRevision: 'catalog-1',
    diagnostics: [],
    skills,
    truncated: false,
    ...overrides
  }
}

type ChatComposerProps = ComponentProps<typeof ChatComposer>

function TestComposer({
  initialDraft = createComposerDraft({ modelId: 'model-1', projectId: 'project-a' }),
  skillCatalogRefreshToken = 0,
  showProjectSelector = false
}: {
  initialDraft?: ChatComposerProps['draft']
  skillCatalogRefreshToken?: number
  showProjectSelector?: boolean
}) {
  const [draft, setDraft] = useState(initialDraft)
  return (
    <div style={{ margin: 120, width: 620 }}>
      <ChatComposer
        draft={draft}
        onDraftChange={(nextDraft) => {
          draftChangeSpy(nextDraft)
          setDraft(nextDraft)
        }}
        onSubmitMessage={submitSpy}
        skillCatalogRefreshToken={skillCatalogRefreshToken}
        showProjectSelector={showProjectSelector}
      />
    </div>
  )
}

beforeEach(() => {
  draftChangeSpy.mockReset()
  listSkillsSpy.mockReset()
  listSkillsSpy.mockImplementation(async (projectId: string) =>
    projectId === 'project-b' ? catalog([projectBSkill, bundledAuditorSkill]) : catalog()
  )
  submitSpy.mockReset()
})

describe('Skill selection invariants', () => {
  it('fails closed for unsupported source and trust contracts', () => {
    expect(() =>
      skillCatalog.assertSupportedSkillCatalog(
        catalog([auditorSkill, bundledAuditorSkill, installedAuditorSkill])
      )
    ).not.toThrow()

    const mismatchedTrust = {
      ...bundledAuditorSkill,
      trust: 'untrusted'
    } as unknown as SkillDescriptor
    const unknownSource = {
      ...bundledAuditorSkill,
      source: { id: 'remote:catalog', kind: 'remote' }
    } as unknown as SkillDescriptor
    const trustedInstalled = {
      ...installedAuditorSkill,
      trust: 'application'
    } as unknown as SkillDescriptor

    expect(() => skillCatalog.assertSupportedSkillCatalog(catalog([mismatchedTrust]))).toThrow(
      'bundled/untrusted/run'
    )
    expect(() => skillCatalog.assertSupportedSkillCatalog(catalog([unknownSource]))).toThrow(
      'remote/application/run'
    )
    expect(() => skillCatalog.assertSupportedSkillCatalog(catalog([trustedInstalled]))).toThrow(
      'installed/application/run'
    )

    const previousSchema = { ...catalog(), schemaVersion: 3 } as unknown as SkillsListOutput
    expect(() => skillCatalog.assertSupportedSkillCatalog(previousSchema)).toThrow(
      'Unsupported Skill catalog schema: 3'
    )
  })

  it('normalizes stored selections with a stable order, unique ids, and a hard limit', () => {
    const stored = Array.from({ length: 10 }, (_, index) => ({
      id: `workspace:project:skill-${index}`,
      revision: `revision-${index}`
    }))
    stored.splice(2, 0, {
      id: 'workspace:project:skill-0',
      revision: 'newer-but-duplicate'
    })

    const normalized = skillSelection.parseStoredSkillSelections(JSON.stringify(stored))

    expect(normalized).toHaveLength(skillSelection.MAX_SELECTED_SKILLS)
    expect(normalized.map((selection) => selection.id)).toEqual(
      Array.from(
        { length: skillSelection.MAX_SELECTED_SKILLS },
        (_, index) => `workspace:project:skill-${index}`
      )
    )
    expect(normalized[0]?.revision).toBe('revision-0')
  })

  it('drops malformed draft JSON and invalid entries without throwing', () => {
    expect(skillSelection.parseStoredSkillSelections('{broken')).toEqual([])
    expect(
      skillSelection.parseStoredSkillSelections(
        JSON.stringify([
          null,
          { id: '', revision: 'revision' },
          { id: 'workspace:project:invalid-whitespace', revision: ' revision ' },
          { id: 'workspace:project:valid', revision: 'revision' },
          { id: 'workspace:project:missing-revision' }
        ])
      )
    ).toEqual([{ id: 'workspace:project:valid', revision: 'revision' }])
  })

  it('distinguishes current, stale, and unavailable selections', () => {
    expect(
      skillSelection.matchSkillSelection({ id: auditorSkill.id, revision: auditorSkill.revision }, [
        auditorSkill
      ]).status
    ).toBe('current')
    expect(
      skillSelection.matchSkillSelection({ id: auditorSkill.id, revision: 'old-revision' }, [
        auditorSkill
      ]).status
    ).toBe('stale')
    expect(
      skillSelection.matchSkillSelection({ id: 'missing', revision: 'revision' }, [auditorSkill])
        .status
    ).toBe('unavailable')
  })

  it('restores an in-flight selection before newer draft selections without reordering it', () => {
    expect(
      skillSelection.mergeSkillSelections(
        [
          { id: 'workspace:project:submitted-a', revision: 'revision-a' },
          { id: 'workspace:project:submitted-b', revision: 'revision-b' }
        ],
        [
          { id: 'workspace:project:new-draft', revision: 'revision-new' },
          { id: 'workspace:project:submitted-a', revision: 'ignored-duplicate' }
        ]
      )
    ).toEqual([
      { id: 'workspace:project:submitted-a', revision: 'revision-a' },
      { id: 'workspace:project:submitted-b', revision: 'revision-b' },
      { id: 'workspace:project:new-draft', revision: 'revision-new' }
    ])
  })
})

describe('Composer permission confirmation', () => {
  it('keeps the current permission until full access is explicitly confirmed', async () => {
    const screen = await render(<TestComposer />)

    await screen.getByRole('button', { name: /chat\.permission/ }).click()
    await screen.getByRole('option', { name: 'chat.fullPermission' }).click()

    await expect
      .element(screen.getByRole('heading', { name: 'chat.fullPermissionConfirmTitle' }))
      .toBeVisible()
    await expect
      .element(screen.getByText('chat.fullPermissionConfirmDescription', { exact: true }))
      .toBeVisible()
    expect(
      draftChangeSpy.mock.calls.some(([nextDraft]) => nextDraft.permissionMode === 'full')
    ).toBe(false)

    await screen
      .getByRole('button', { name: 'chat.fullPermissionConfirmCancel', exact: true })
      .nth(1)
      .click()
    await expect.poll(() => document.querySelector('[role="dialog"]')).toBeNull()
    expect(
      draftChangeSpy.mock.calls.some(([nextDraft]) => nextDraft.permissionMode === 'full')
    ).toBe(false)

    await screen.getByRole('button', { name: /chat\.permission/ }).click()
    await screen.getByRole('option', { name: 'chat.fullPermission' }).click()
    await screen
      .getByRole('button', { name: 'chat.fullPermissionConfirmAction', exact: true })
      .click()

    await expect
      .poll(() =>
        draftChangeSpy.mock.calls.some(([nextDraft]) => nextDraft.permissionMode === 'full')
      )
      .toBe(true)
    await expect
      .element(screen.getByRole('button', { name: /chat\.permission.*chat\.fullPermission/ }))
      .toBeVisible()
  })
})

describe('ChatComposer Skill picker', () => {
  it('never renders a ready catalog from a different project scope', async () => {
    const screen = await render(
      <ComposerSkillPicker
        catalogState={{ status: 'ready', projectId: 'project-a', output: catalog() }}
        onClose={vi.fn()}
        onRefresh={vi.fn()}
        onSearchChange={vi.fn()}
        onToggle={vi.fn()}
        onUseLatest={vi.fn()}
        projectId="project-b"
        search=""
        selections={[]}
      />
    )

    expect(screen.container.textContent).not.toContain(auditorSkill.name)
    expect(screen.container.textContent).not.toContain(testSkill.name)
    expect(screen.container.querySelector('[role="list"]')).toBeNull()
  })

  it('renders loading, truncated diagnostics, and empty catalog states explicitly', async () => {
    let resolveCatalog: ((output: SkillsListOutput) => void) | undefined
    listSkillsSpy.mockReturnValue(
      new Promise<SkillsListOutput>((resolve) => {
        resolveCatalog = resolve
      })
    )
    const screen = await render(<TestComposer />)

    await screen.getByRole('button', { name: 'chat.addContext' }).click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()
    await expect.element(screen.getByRole('status')).toHaveTextContent('chat.loadingSkills')

    resolveCatalog?.(
      catalog([], {
        diagnostics: [
          {
            code: 'invalidFrontmatter',
            location: '.agents/skills/broken/SKILL.md',
            message: 'Broken frontmatter',
            severity: 'error'
          }
        ],
        truncated: true
      })
    )

    await expect
      .element(screen.getByText('chat.skillCatalogTruncated', { exact: true }))
      .toBeVisible()
    await expect.element(screen.getByText('chat.skillDiagnostics', { exact: true })).toBeVisible()
    await expect.element(screen.getByText('chat.noSkills', { exact: true })).toBeVisible()
  })

  it('shows catalog failures and lets the user retry', async () => {
    listSkillsSpy
      .mockRejectedValueOnce(new Error('catalog offline'))
      .mockResolvedValueOnce(catalog([]))
    const screen = await render(<TestComposer />)

    await screen.getByRole('button', { name: 'chat.addContext' }).click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()
    await expect.element(screen.getByRole('alert')).toHaveTextContent('catalog offline')
    await screen.getByRole('button', { name: /chat.retrySkills/ }).click()

    await expect.poll(() => listSkillsSpy.mock.calls.length).toBe(2)
    await expect.element(screen.getByText('chat.noSkills', { exact: true })).toBeVisible()
  })

  it('reloads an enabled catalog when the host recovery refresh token changes', async () => {
    const initialDraft = createComposerDraft({
      modelId: 'model-1',
      projectId: 'project-a',
      skills: [{ id: auditorSkill.id, revision: auditorSkill.revision }]
    })
    const screen = await render(
      <TestComposer initialDraft={initialDraft} skillCatalogRefreshToken={0} />
    )

    await expect.poll(() => listSkillsSpy.mock.calls.length).toBe(1)
    await screen.rerender(<TestComposer initialDraft={initialDraft} skillCatalogRefreshToken={1} />)

    await expect.poll(() => listSkillsSpy.mock.calls.length).toBe(2)
  })

  it('selects skills in order, submits structured selections, and never writes Skill text', async () => {
    const screen = await render(<TestComposer />)

    await screen.getByRole('button', { name: 'chat.addContext' }).click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()
    await expect.poll(() => listSkillsSpy.mock.calls.length).toBe(1)
    expect(listSkillsSpy).toHaveBeenCalledWith('project-a')

    await screen.getByRole('button', { name: /^Repository auditor/ }).click()
    await screen.getByRole('button', { name: /^Test runner/ }).click()
    await expect
      .element(
        screen.getByRole('button', {
          name: 'chat.removeSkill Repository auditor · chat.workspaceSkill · chat.skillTrustUntrusted'
        })
      )
      .toBeVisible()
    await expect
      .element(
        screen.getByRole('button', {
          name: 'chat.removeSkill Test runner · chat.workspaceSkill · chat.skillTrustUntrusted'
        })
      )
      .toBeVisible()

    await screen.getByRole('textbox', { name: 'chat.inputAria' }).fill('Inspect this repository')
    await screen.getByRole('button', { name: 'chat.send' }).click()
    await expect.poll(() => submitSpy.mock.calls.length).toBe(1)

    const [message, options] = submitSpy.mock.calls[0] ?? []
    expect(message).toBe('Inspect this repository')
    expect(message).not.toContain('Repository auditor')
    expect(message).not.toContain('SKILL.md')
    expect(options.skills).toEqual([
      { id: auditorSkill.id, revision: auditorSkill.revision },
      { id: testSkill.id, revision: testSkill.revision }
    ])
    await expect
      .poll(() => screen.container.querySelectorAll('.composer-skill-chip').length)
      .toBe(0)
  })

  it('displays registered source and trust metadata and submits opaque selections', async () => {
    listSkillsSpy.mockResolvedValue(
      catalog([auditorSkill, testSkill, bundledAuditorSkill, installedAuditorSkill])
    )
    const screen = await render(<TestComposer />)

    await screen.getByRole('button', { name: 'chat.addContext' }).click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()

    await expect.element(screen.getByText(/chat\.bundledSkill/)).toBeVisible()
    await expect.element(screen.getByText(/chat\.installedSkill/)).toBeVisible()
    await expect.element(screen.getByText(/chat\.skillTrustApplication/)).toBeVisible()
    expect(screen.container.textContent).toContain('chat.workspaceSkill')
    expect(screen.container.textContent).toContain('chat.skillTrustUntrusted')

    await screen.getByRole('button', { name: /^Repository evidence auditor/ }).click()
    await screen.getByRole('textbox', { name: 'chat.inputAria' }).fill('Audit this claim')
    await screen.getByRole('button', { name: 'chat.send' }).click()
    await expect.poll(() => submitSpy.mock.calls.length).toBe(1)

    expect(submitSpy.mock.calls[0]?.[1].skills).toEqual([
      { id: bundledAuditorSkill.id, revision: bundledAuditorSkill.revision }
    ])
  })

  it('keeps same-named selected skills visibly and accessibly distinct by provenance', async () => {
    const sameNamedBundledSkill = {
      ...bundledAuditorSkill,
      name: auditorSkill.name
    }
    const sameNamedInstalledSkill = {
      ...installedAuditorSkill,
      name: auditorSkill.name
    }
    listSkillsSpy.mockResolvedValue(
      catalog([auditorSkill, sameNamedBundledSkill, sameNamedInstalledSkill])
    )
    const screen = await render(
      <TestComposer
        initialDraft={createComposerDraft({
          modelId: 'model-1',
          projectId: 'project-a',
          skills: [
            { id: auditorSkill.id, revision: auditorSkill.revision },
            { id: sameNamedBundledSkill.id, revision: sameNamedBundledSkill.revision },
            { id: sameNamedInstalledSkill.id, revision: sameNamedInstalledSkill.revision }
          ]
        })}
      />
    )

    await expect
      .poll(() => screen.container.querySelectorAll('.composer-skill-chip').length)
      .toBe(3)
    const workspaceChip = screen.container.querySelector(
      '.composer-skill-chip[data-source-kind="workspace"][data-trust="untrusted"]'
    )
    const bundledChip = screen.container.querySelector(
      '.composer-skill-chip[data-source-kind="bundled"][data-trust="application"]'
    )
    const installedChip = screen.container.querySelector(
      '.composer-skill-chip[data-source-kind="installed"][data-trust="untrusted"]'
    )

    expect(workspaceChip?.textContent).toContain(auditorSkill.name)
    expect(workspaceChip?.textContent).toContain('chat.workspaceSkill · chat.skillTrustUntrusted')
    expect(bundledChip?.textContent).toContain(auditorSkill.name)
    expect(bundledChip?.textContent).toContain('chat.bundledSkill · chat.skillTrustApplication')
    expect(installedChip?.textContent).toContain(auditorSkill.name)
    expect(installedChip?.textContent).toContain('chat.installedSkill · chat.skillTrustUntrusted')
    await expect
      .element(
        screen.getByRole('button', {
          name: 'chat.removeSkill Repository auditor · chat.workspaceSkill · chat.skillTrustUntrusted'
        })
      )
      .toBeVisible()
    await expect
      .element(
        screen.getByRole('button', {
          name: 'chat.removeSkill Repository auditor · chat.installedSkill · chat.skillTrustUntrusted'
        })
      )
      .toBeVisible()
    await expect
      .element(
        screen.getByRole('button', {
          name: 'chat.removeSkill Repository auditor · chat.bundledSkill · chat.skillTrustApplication'
        })
      )
      .toBeVisible()
  })

  it('keeps a stale revision selected and requires an explicit update before sending', async () => {
    const oldRevision = 'skill-sha256-v1:old'
    const screen = await render(
      <TestComposer
        initialDraft={createComposerDraft({
          message: 'Run the audit',
          modelId: 'model-1',
          projectId: 'project-a',
          skills: [{ id: auditorSkill.id, revision: oldRevision }]
        })}
      />
    )

    await expect.poll(() => listSkillsSpy.mock.calls.length).toBe(1)
    await expect.element(screen.getByRole('alert')).toHaveTextContent('chat.skillStaleDescription')
    await expect.element(screen.getByRole('button', { name: 'chat.send' })).toBeDisabled()
    expect(
      draftChangeSpy.mock.calls.find(
        ([draft]) => draft.skills[0]?.revision === auditorSkill.revision
      )
    ).toBeUndefined()

    await screen.getByRole('button', { name: 'chat.addContext' }).click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()
    await screen.getByRole('button', { name: 'chat.useLatestSkill' }).click()

    await expect.element(screen.getByRole('button', { name: 'chat.send' })).toBeEnabled()
    const updatedDraft = draftChangeSpy.mock.calls.at(-1)?.[0]
    expect(updatedDraft.skills).toEqual([{ id: auditorSkill.id, revision: auditorSkill.revision }])
  })

  it('uses native toggle buttons and restores trigger focus after Escape and close', async () => {
    const screen = await render(<TestComposer />)
    const addContextButton = screen.getByRole('button', { name: 'chat.addContext' })

    await addContextButton.click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()

    const searchInput = screen.getByRole('textbox', { name: 'chat.searchSkills' })
    await expect.element(searchInput).toHaveFocus()
    await expect.element(screen.getByRole('list', { name: 'chat.skills' })).toBeVisible()

    const auditorToggle = screen.getByRole('button', { name: /^Repository auditor/ })
    await expect.element(auditorToggle).toHaveAttribute('aria-pressed', 'false')
    await userEvent.keyboard('{Tab}')
    await expect.element(auditorToggle).toHaveFocus()
    await userEvent.keyboard('{Enter}')
    await expect.element(auditorToggle).toHaveAttribute('aria-pressed', 'true')

    await userEvent.keyboard('{Escape}')
    await expect.poll(() => screen.container.querySelector('[role="dialog"]')).toBeNull()
    await expect.element(addContextButton).toHaveFocus()

    await addContextButton.click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()
    const closeButton = screen.getByRole('button', { name: 'chat.closeSkills' })
    ;(closeButton.element() as HTMLButtonElement).focus()
    await expect.element(closeButton).toHaveFocus()
    await userEvent.keyboard('{Enter}')
    await expect.poll(() => screen.container.querySelector('[role="dialog"]')).toBeNull()
    await expect.element(addContextButton).toHaveFocus()
  })

  it('preserves the target scope Skill draft when conversation and project change together', async () => {
    const draftA = createComposerDraft({
      modelId: 'model-1',
      projectId: 'project-a',
      skills: [{ id: auditorSkill.id, revision: auditorSkill.revision }]
    })
    const draftB = createComposerDraft({
      modelId: 'model-1',
      projectId: 'project-b',
      skills: [{ id: projectBSkill.id, revision: projectBSkill.revision }]
    })
    const renderScope = (resetKey: string, draft: ChatComposerProps['draft']) => (
      <div style={{ margin: 120, width: 620 }}>
        <ChatComposer
          draft={draft}
          onDraftChange={draftChangeSpy}
          onSubmitMessage={submitSpy}
          resetKey={resetKey}
        />
      </div>
    )
    const screen = await render(renderScope('conversation-a', draftA))

    await expect
      .poll(() => listSkillsSpy.mock.calls.some(([projectId]) => projectId === 'project-a'))
      .toBe(true)
    draftChangeSpy.mockReset()

    await screen.rerender(renderScope('conversation-b', draftB))
    await expect
      .poll(() => listSkillsSpy.mock.calls.some(([projectId]) => projectId === 'project-b'))
      .toBe(true)

    await expect
      .element(
        screen.getByRole('button', {
          name: 'chat.removeSkill Dependency auditor · chat.workspaceSkill · chat.skillTrustUntrusted'
        })
      )
      .toBeVisible()
    expect(draftChangeSpy).not.toHaveBeenCalled()
  })

  it('clears workspace Skill selections before switching projects', async () => {
    const screen = await render(
      <TestComposer
        initialDraft={createComposerDraft({
          message: 'Inspect the other project',
          modelId: 'model-1',
          projectId: 'project-a',
          skills: [{ id: auditorSkill.id, revision: auditorSkill.revision }]
        })}
        showProjectSelector
      />
    )

    await screen.getByRole('button', { name: /Project A/ }).click()
    await screen.getByRole('option', { name: /Project B/ }).click()
    await expect
      .poll(() => screen.container.querySelectorAll('.composer-skill-chip').length)
      .toBe(0)

    await screen.getByRole('button', { name: 'chat.send' }).click()
    await expect.poll(() => submitSpy.mock.calls.length).toBe(1)
    expect(submitSpy.mock.calls[0]?.[1].projectId).toBe('project-b')
    expect(submitSpy.mock.calls[0]?.[1].skills).toEqual([])
  })
})
