import { useRef, useState } from 'react'
import type { ComponentProps } from 'react'
import type { SkillDescriptor, SkillsListOutput } from '@mycopilot/protocol'
import { userEvent } from 'vitest/browser'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import '../../styles/global.css'
import '../../features/chat/components/ChatComposer.css'

const { draftChangeSpy, listSkillsSpy, submitSpy, translate } = vi.hoisted(() => ({
  draftChangeSpy: vi.fn(),
  listSkillsSpy: vi.fn(),
  submitSpy: vi.fn(),
  translate: (key: string) => key
}))

vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: translate })
}))

vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({
    enabledModels: [
      {
        id: 'model-1',
        providerModelId: 'provider-model-one',
        displayName: 'Model One',
        supportsImage: true,
        inputPrice: '0',
        outputPrice: '0',
        enabled: true
      },
      ...Array.from({ length: 6 }, (_, index) => ({
        id: `model-${index + 2}`,
        providerModelId: `provider-model-${index + 2}`,
        displayName: `Model ${index + 2}`,
        supportsImage: index % 2 === 0,
        inputPrice: '0',
        outputPrice: '0',
        enabled: true
      }))
    ]
  })
}))

vi.mock('../../config/ProjectSettingsProvider', async () => {
  const { singleFolderProject } = await import('../../features/projects/__tests__/projectFixtures')
  return {
    useProjectSettings: () => ({
      projects: [
        singleFolderProject({ id: 'project-a', name: 'Project A', path: '/workspace/a' }),
        singleFolderProject({
          id: 'project-b',
          name: 'Project B',
          path: '/workspace/b',
          createdAt: 2
        })
      ],
      openCreateProjectDialog: vi.fn(async () => null)
    })
  }
})

vi.mock('../../features/chat/components/ImagePreview', () => ({
  useImagePreview: () => vi.fn()
}))

vi.mock('../../features/chat/chatAttachments', () => ({
  buildAgentInputAttachments: (attachments: unknown[]) => attachments,
  composerAttachmentFromAgentAttachment: (attachment: unknown) => attachment,
  createComposerAttachmentsFromFiles: async () => [],
  loadComposerAttachmentImage: async () => undefined,
  loadComposerAttachmentPreview: async () => undefined,
  createAttachmentSummary: () => '',
  selectComposerAttachments: async () => [],
  stripAttachmentSummary: (content: string) => content
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

const bundledDocumentsSkill: SkillDescriptor = {
  activationScope: 'run',
  description: 'Create and edit document files.',
  id: 'bundled:application:documents',
  location: 'skills/documents/SKILL.md',
  name: 'Documents',
  revision: 'skill-package-sha256-v3:bundled-documents',
  source: { id: 'application:documents', kind: 'bundled' },
  trust: 'application'
}

const bundledImageGenerationSkill: SkillDescriptor = {
  ...bundledDocumentsSkill,
  description: 'Generate or edit image artifacts.',
  id: 'bundled:application:image-generation',
  location: 'skills/image-generation/SKILL.md',
  name: 'Image Generation',
  source: { id: 'application:image-generation', kind: 'bundled' }
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

const bundledOfficeSkills: SkillDescriptor[] = [
  bundledDocumentsSkill,
  {
    ...bundledDocumentsSkill,
    description: 'Create and edit spreadsheet files.',
    id: 'bundled:application:spreadsheets',
    location: 'skills/spreadsheets/SKILL.md',
    name: 'Spreadsheets',
    source: { id: 'application:spreadsheets', kind: 'bundled' }
  },
  {
    ...bundledDocumentsSkill,
    description: 'Create and edit presentation files.',
    id: 'bundled:application:presentations',
    location: 'skills/presentations/SKILL.md',
    name: 'Presentations',
    source: { id: 'application:presentations', kind: 'bundled' }
  }
]

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
  isGenerating = false,
  isModelTransitionRunning = false,
  onSubmitMessage = submitSpy,
  onGuideQueuedMessage,
  initialQueueAutoSendEnabled = false,
  onToggleQueueAutoSend,
  skillCatalogRefreshToken = 0,
  showProjectSelector = false
}: {
  initialDraft?: ChatComposerProps['draft']
  isGenerating?: boolean
  isModelTransitionRunning?: boolean
  onSubmitMessage?: ChatComposerProps['onSubmitMessage']
  onGuideQueuedMessage?: ChatComposerProps['onGuideQueuedMessage']
  initialQueueAutoSendEnabled?: boolean
  onToggleQueueAutoSend?: ChatComposerProps['onToggleQueueAutoSend']
  skillCatalogRefreshToken?: number
  showProjectSelector?: boolean
}) {
  const [draft, setDraft] = useState(initialDraft)
  const [queueAutoSendEnabled, setQueueAutoSendEnabled] = useState(initialQueueAutoSendEnabled)
  return (
    <div style={{ margin: 120, width: 620 }}>
      <ChatComposer
        canGuideQueuedMessages={isGenerating}
        draft={draft}
        isGenerating={isGenerating}
        isModelTransitionRunning={isModelTransitionRunning}
        onDraftChange={(nextDraft) => {
          draftChangeSpy(nextDraft)
          setDraft(nextDraft)
        }}
        onSubmitMessage={onSubmitMessage}
        onGuideQueuedMessage={onGuideQueuedMessage}
        queueAutoSendEnabled={queueAutoSendEnabled}
        onToggleQueueAutoSend={
          onToggleQueueAutoSend
            ? () => {
                onToggleQueueAutoSend()
                setQueueAutoSendEnabled((enabled) => !enabled)
              }
            : undefined
        }
        skillCatalogRefreshToken={skillCatalogRefreshToken}
        showProjectSelector={showProjectSelector}
      />
    </div>
  )
}

function RefPersistedMessageComposer() {
  const [renderedDraft, setRenderedDraft] = useState(() =>
    createComposerDraft({ modelId: 'model-1', projectId: 'project-a' })
  )
  const authoritativeDraftRef = useRef(renderedDraft)
  const [messageSyncKey, setMessageSyncKey] = useState('user-before-transition')

  return (
    <div style={{ margin: 120, width: 620 }}>
      <ChatComposer
        draft={renderedDraft}
        messageSyncKey={messageSyncKey}
        onDraftChange={(nextDraft) => {
          authoritativeDraftRef.current = nextDraft
          setRenderedDraft(nextDraft)
        }}
        onDraftMessageChange={(nextDraft) => {
          // Match AppShell's keystroke fast path: persist the authoritative value without forcing
          // the whole shell to rerender.
          authoritativeDraftRef.current = nextDraft
        }}
        onSubmitMessage={() => false}
      />
      <button
        type="button"
        onClick={() => setRenderedDraft((currentDraft) => ({ ...currentDraft }))}
      >
        rerender-stale-draft
      </button>
      <button
        type="button"
        onClick={() => {
          const currentDraft = authoritativeDraftRef.current
          const committedDraft = {
            ...currentDraft,
            message: '',
            updatedAt: Math.max(Date.now(), currentDraft.updatedAt + 1)
          }
          authoritativeDraftRef.current = committedDraft
          setRenderedDraft(committedDraft)
          setMessageSyncKey('user-after-transition')
        }}
      >
        commit-user-message
      </button>
      <button
        type="button"
        onClick={() => {
          const currentDraft = authoritativeDraftRef.current
          const committedDraft = {
            ...currentDraft,
            updatedAt: Math.max(Date.now(), currentDraft.updatedAt + 1)
          }
          authoritativeDraftRef.current = committedDraft
          setRenderedDraft(committedDraft)
          setMessageSyncKey('queued-user-after-transition')
        }}
      >
        commit-queued-user-message
      </button>
    </div>
  )
}

beforeEach(() => {
  draftChangeSpy.mockReset()
  listSkillsSpy.mockReset()
  listSkillsSpy.mockImplementation(async (projectId: string | null) => {
    if (projectId === null) {
      return catalog([bundledDocumentsSkill, bundledImageGenerationSkill, installedAuditorSkill])
    }
    return projectId === 'project-b' ? catalog([projectBSkill, bundledDocumentsSkill]) : catalog()
  })
  submitSpy.mockReset()
})

describe('ChatComposer model picker', () => {
  it('shows five compact rows and scrolls additional models', async () => {
    const screen = await render(<TestComposer />)

    await screen.getByRole('button', { name: 'chat.selectModel' }).click()

    const menu = screen.getByRole('listbox', { name: 'chat.selectModel' }).element() as HTMLElement
    const options = Array.from(menu.querySelectorAll<HTMLElement>('.composer-model-option'))

    expect(options).toHaveLength(7)
    expect(getComputedStyle(menu).overflowY).toBe('auto')
    expect(menu.scrollHeight).toBeGreaterThan(menu.clientHeight)
    const visibleBottom = menu.getBoundingClientRect().bottom
    expect(options[4].getBoundingClientRect().bottom).toBeLessThanOrEqual(visibleBottom)
    expect(options[5].getBoundingClientRect().bottom).toBeGreaterThan(visibleBottom)
    expect(options[0]?.getBoundingClientRect().height).toBeLessThanOrEqual(32)
  })

  it('updates only the composer draft when a model is selected', async () => {
    const screen = await render(<TestComposer />)

    await screen.getByRole('button', { name: 'chat.selectModel' }).click()
    await screen.getByRole('option', { name: /Model 2/ }).click()

    expect(draftChangeSpy).toHaveBeenCalledWith(expect.objectContaining({ modelId: 'model-2' }))
    await expect
      .element(screen.getByRole('button', { name: 'chat.selectModel' }))
      .toHaveTextContent('Model 2')
  })

  it('locks text input and model selection while history is being compacted', async () => {
    const screen = await render(<TestComposer isModelTransitionRunning />)

    await expect.element(screen.getByRole('textbox', { name: 'chat.inputAria' })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: 'chat.selectModel' })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: /chat\.permission/ })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: 'chat.send' })).toBeDisabled()
  })

  it('preserves a newer next-turn selection when an older submission finishes', async () => {
    let finishSubmission!: (accepted: boolean) => void
    const pendingSubmit = vi.fn<NonNullable<ChatComposerProps['onSubmitMessage']>>(
      () =>
        new Promise<boolean>((resolve) => {
          finishSubmission = resolve
        })
    )
    const screen = await render(<TestComposer onSubmitMessage={pendingSubmit} />)
    const input = screen.getByRole('textbox', { name: 'chat.inputAria' })
    await input.fill('start the current turn')
    await input.click()
    await userEvent.keyboard('{Enter}')
    expect(pendingSubmit).toHaveBeenCalledWith(
      'start the current turn',
      expect.objectContaining({
        modelId: 'model-1',
        permissionMode: 'default'
      })
    )
    await screen.rerender(<TestComposer isGenerating onSubmitMessage={pendingSubmit} />)
    await screen.getByRole('button', { name: 'chat.selectModel' }).click()
    await screen.getByRole('option', { name: /Model 2/ }).click()
    await screen.getByRole('button', { name: /chat\.permission/ }).click()
    await screen.getByRole('option', { name: 'chat.customPermission' }).click()
    finishSubmission(true)
    // Only the owner commits a draft clear; a late boolean result must not mutate it.
    await expect.element(input).toHaveValue('start the current turn')
    expect(draftChangeSpy.mock.lastCall?.[0]).toMatchObject({
      modelId: 'model-2',
      permissionMode: 'custom'
    })
    await expect
      .element(screen.getByRole('button', { name: 'chat.selectModel' }))
      .toHaveTextContent('Model 2')
    await expect
      .element(screen.getByRole('button', { name: /chat\.permission/ }))
      .toHaveTextContent('chat.customPermission')
    expect(pendingSubmit).toHaveBeenCalledTimes(1)
  })

  it('clears only unchanged submitted fields when a newer draft is composed during submission', async () => {
    let finishSubmission!: (accepted: boolean) => void
    const pendingSubmit = vi.fn<NonNullable<ChatComposerProps['onSubmitMessage']>>(
      () =>
        new Promise<boolean>((resolve) => {
          finishSubmission = resolve
        })
    )
    const initialDraft = createComposerDraft({
      modelId: 'model-1',
      projectId: 'project-a',
      message: 'original submission',
      skills: [{ id: auditorSkill.id, revision: auditorSkill.revision }],
      attachments: ['remove.txt', 'keep.txt'].map((name) => ({
        id: name,
        kind: 'file' as const,
        name,
        mimeType: 'text/plain',
        sizeBytes: 4,
        encoding: 'managed' as const,
        data: 'managed-test-a2VlcA=='
      }))
    })
    const screen = await render(
      <TestComposer
        initialDraft={initialDraft}
        showProjectSelector
        onSubmitMessage={pendingSubmit}
      />
    )
    const input = screen.getByRole('textbox', { name: 'chat.inputAria' })
    await screen.getByRole('button', { name: 'chat.send' }).click()
    await expect.poll(() => pendingSubmit.mock.calls.length).toBe(1)
    await input.fill('new input while the previous send is pending')
    await screen.getByRole('button', { name: 'chat.removeAttachment remove.txt' }).click()
    await screen.getByRole('button', { name: 'Project A' }).click()
    await screen.getByRole('option', { name: /Project B/ }).click()
    await screen.getByRole('button', { name: 'chat.addContext' }).click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()
    await screen.getByRole('button', { name: /^Dependency auditor/ }).click()
    finishSubmission(true)
    await expect
      .poll(() => draftChangeSpy.mock.lastCall?.[0])
      .toMatchObject({
        message: 'new input while the previous send is pending',
        projectId: 'project-b',
        skills: [{ id: projectBSkill.id, revision: projectBSkill.revision }],
        attachments: [initialDraft.attachments[1]]
      })
    await expect.element(input).toHaveValue('new input while the previous send is pending')
    await expect.element(screen.getByText('keep.txt')).toBeVisible()
    expect(pendingSubmit).toHaveBeenCalledTimes(1)
    expect(pendingSubmit.mock.calls[0]?.[1]).toMatchObject({
      projectId: 'project-a',
      skills: initialDraft.skills,
      attachments: initialDraft.attachments
    })
  })

  it('clears ref-persisted input with the optimistic message and preserves identical new text after late acceptance', async () => {
    const { consumeSubmittedDraft } = await import('../composerSubmission')
    const versions: Array<[number, number]> = []
    let finish!: (accepted: boolean) => void
    const acceptance = new Promise<boolean>((resolve) => {
      finish = resolve
    })
    function Owner() {
      const [draft, setDraft] = useState(() =>
        createComposerDraft({ modelId: 'model-1', projectId: 'project-a' })
      )
      const liveDraft = useRef(draft)
      const [posted, setPosted] = useState('')
      return (
        <>
          <output data-testid="optimistic-message">{posted}</output>
          <ChatComposer
            draft={draft}
            onDraftChange={(next) => {
              liveDraft.current = next
              setDraft(next)
            }}
            onDraftMessageChange={(next) => {
              // Production keeps keystrokes in a ref, without rerendering the parent.
              liveDraft.current = next
            }}
            messageSyncKey={posted}
            onSubmitMessage={async (content, options) => {
              setPosted(content)
              versions.push([liveDraft.current.updatedAt, options.draftSnapshot!.updatedAt])
              const consumed = consumeSubmittedDraft(
                liveDraft.current,
                options.draftSnapshot!,
                options
              )
              liveDraft.current = consumed
              setDraft(consumed)
              return acceptance
            }}
          />
        </>
      )
    }
    const screen = await render(<Owner />)
    const input = screen.getByRole('textbox', { name: 'chat.inputAria' })
    await input.fill('same message')
    await screen.getByRole('button', { name: 'chat.send' }).click()
    await expect.element(screen.getByTestId('optimistic-message')).toHaveTextContent('same message')
    expect(versions[0][1]).toBe(versions[0][0])
    await expect.element(input).toHaveValue('')
    await input.fill('same message')
    finish(true)
    await new Promise((resolve) => window.requestAnimationFrame(resolve))
    await expect.element(input).toHaveValue('same message')
  })

  it('retains composer input when the submit guard defers message creation', async () => {
    const deferredSubmit = vi.fn().mockResolvedValue(false)
    const deferredDraft = createComposerDraft({
      attachments: [
        {
          id: 'attachment-deferred',
          kind: 'file',
          name: 'keep.txt',
          mimeType: 'text/plain',
          sizeBytes: 4,
          encoding: 'managed',
          data: 'managed-test-a2VlcA=='
        }
      ],
      modelId: 'model-1',
      permissionMode: 'custom',
      projectId: 'project-a',
      skills: [{ id: auditorSkill.id, revision: auditorSkill.revision }]
    })
    const screen = await render(
      <TestComposer initialDraft={deferredDraft} onSubmitMessage={deferredSubmit} />
    )
    const input = screen.getByRole('textbox', { name: 'chat.inputAria' })

    await input.fill('keep this message')
    await screen.getByRole('button', { name: 'chat.send' }).click()

    await expect.poll(() => deferredSubmit.mock.calls.length).toBe(1)
    expect(deferredSubmit.mock.calls[0]?.[1]).toMatchObject({
      attachments: deferredDraft.attachments,
      modelId: 'model-1',
      permissionMode: 'custom',
      projectId: 'project-a',
      skills: deferredDraft.skills
    })
    await expect.element(input).toHaveValue('keep this message')
    await expect.element(screen.getByText('keep.txt')).toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: /^chat\.removeSkill Repository auditor/ }))
      .toBeVisible()
  })

  it('clears ref-persisted input when a deferred transition commits the user message', async () => {
    const screen = await render(<RefPersistedMessageComposer />)
    const input = screen.getByRole('textbox', { name: 'chat.inputAria' })

    await input.fill('send after provider transition')
    await screen.getByRole('button', { name: 'chat.send' }).click()
    await expect.element(input).toHaveValue('send after provider transition')

    // A stale parent rerender must not overwrite local keystrokes while the transition is pending.
    await screen.getByRole('button', { name: 'rerender-stale-draft' }).click()
    await expect.element(input).toHaveValue('send after provider transition')

    // The committed user-message identity is the explicit authority boundary for clearing it.
    await screen.getByRole('button', { name: 'commit-user-message' }).click()
    await expect.element(input).toHaveValue('')
  })

  it('keeps a newer ref-persisted draft when a queued message commits', async () => {
    const screen = await render(<RefPersistedMessageComposer />)
    const input = screen.getByRole('textbox', { name: 'chat.inputAria' })

    await input.fill('new draft while the queued message runs')
    await screen.getByRole('button', { name: 'commit-queued-user-message' }).click()

    await expect.element(input).toHaveValue('new draft while the queued message runs')
  })
})

describe('Skill selection invariants', () => {
  it('fails closed for unsupported source and trust contracts', () => {
    expect(() =>
      skillCatalog.assertSupportedSkillCatalog(
        catalog([auditorSkill, bundledDocumentsSkill, installedAuditorSkill])
      )
    ).not.toThrow()

    const mismatchedTrust = {
      ...bundledDocumentsSkill,
      trust: 'untrusted'
    } as unknown as SkillDescriptor
    const unknownSource = {
      ...bundledDocumentsSkill,
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

describe('running composer guidance queue', () => {
  const queuedMessage = (id: string, content: string, createdAt: number) => ({
    id,
    clientMessageId: `client-${id}`,
    content,
    attachments: [],
    modelId: 'model-1',
    permissionMode: 'default' as const,
    projectId: 'project-a',
    skills: [],
    status: 'pending' as const,
    createdAt
  })

  it('captures new selections only in newly queued messages and preserves existing queue configuration', async () => {
    const originalQueue = [queuedMessage('old', 'old queued message', 1)]
    const screen = await render(
      <TestComposer
        isGenerating
        initialDraft={createComposerDraft({
          modelId: 'model-1',
          projectId: 'project-a',
          queuedMessages: originalQueue
        })}
      />
    )
    const model = screen.getByRole('button', { name: 'chat.selectModel' })
    const permission = screen.getByRole('button', { name: /chat\.permission/ })
    await expect.element(model).toHaveAttribute('title', 'chat.nextTurnConfigurationHint')
    await expect.element(permission).toHaveAttribute('title', 'chat.nextTurnConfigurationHint')
    await model.click()
    await screen.getByRole('option', { name: /Model 2/ }).click()
    await permission.click()
    await screen.getByRole('option', { name: 'chat.customPermission' }).click()
    expect(draftChangeSpy.mock.lastCall?.[0].queuedMessages).toEqual(originalQueue)
    const input = screen.getByRole('textbox', { name: 'chat.inputAria' })
    await input.fill('use the next-turn configuration')
    await input.click()
    await userEvent.keyboard('{Enter}')
    await expect.element(input).toHaveValue('')
    const latestDraft = draftChangeSpy.mock.lastCall?.[0] as ChatComposerProps['draft']
    expect(latestDraft).toMatchObject({ modelId: 'model-2', permissionMode: 'custom' })
    expect(latestDraft.queuedMessages).toEqual([
      originalQueue[0],
      expect.objectContaining({
        content: 'use the next-turn configuration',
        modelId: 'model-2',
        permissionMode: 'custom',
        status: 'pending'
      })
    ])
    expect(submitSpy).not.toHaveBeenCalled()
  })

  it('allows guiding any selected row without consuming rows above it', async () => {
    const guideSpy = vi.fn()
    const screen = await render(
      <TestComposer
        initialDraft={createComposerDraft({
          modelId: 'model-1',
          projectId: 'project-a',
          queuedMessages: [
            queuedMessage('one', '123', 1),
            queuedMessage('two', '345', 2),
            queuedMessage('three', '890', 3)
          ]
        })}
        isGenerating
        onGuideQueuedMessage={guideSpy}
      />
    )

    await screen.getByRole('button', { name: 'chat.guideCurrentRun' }).nth(2).click()

    expect(guideSpy).toHaveBeenCalledWith(expect.objectContaining({ id: 'three', content: '890' }))
    await expect.element(screen.getByText('123')).toBeVisible()
    await expect.element(screen.getByText('345')).toBeVisible()
  })

  it('toggles auto-send for the whole queue from any row menu without editing its messages', async () => {
    const toggleSpy = vi.fn()
    const screen = await render(
      <TestComposer
        initialDraft={createComposerDraft({
          modelId: 'model-1',
          projectId: 'project-a',
          queuedMessages: [
            queuedMessage('one', 'first queued message', 1),
            queuedMessage('two', 'second queued message', 2)
          ]
        })}
        isGenerating
        onToggleQueueAutoSend={toggleSpy}
      />
    )

    await screen.getByRole('button', { name: 'chat.queuedMessageMenu' }).first().click()
    await expect
      .element(screen.getByRole('menuitem', { name: 'chat.editQueuedMessage' }))
      .toBeVisible()
    expect(screen.container.querySelectorAll('[role="menuitem"]')).toHaveLength(2)
    expect(screen.container.textContent).not.toContain('chat.openQueuedMessageInSideChat')
    await screen.getByRole('menuitem', { name: 'chat.enableQueueAutoSend' }).click()
    expect(toggleSpy).toHaveBeenCalledTimes(1)
    expect(screen.container.querySelector('[role="menu"]')).toBeNull()

    await screen.getByRole('button', { name: 'chat.queuedMessageMenu' }).nth(1).click()
    await screen.getByRole('menuitem', { name: 'chat.disableQueueAutoSend' }).click()
    expect(toggleSpy).toHaveBeenCalledTimes(2)

    await screen.getByRole('button', { name: 'chat.queuedMessageMenu' }).first().click()
    await expect
      .element(screen.getByRole('menuitem', { name: 'chat.enableQueueAutoSend' }))
      .toBeVisible()
    await expect.element(screen.getByText('first queued message')).toBeVisible()
    await expect.element(screen.getByText('second queued message')).toBeVisible()
    expect(draftChangeSpy).not.toHaveBeenCalled()
    expect(submitSpy).not.toHaveBeenCalled()
  })

  it('moves an edited row back to the composer and overwrites existing input', async () => {
    const screen = await render(
      <TestComposer
        initialDraft={createComposerDraft({
          message: 'replace me',
          modelId: 'model-1',
          projectId: 'project-a',
          queuedMessages: [queuedMessage('one', 'queued content', 1)]
        })}
        isGenerating
      />
    )

    await screen.getByRole('button', { name: 'chat.queuedMessageMenu' }).click()
    await screen.getByRole('menuitem', { name: 'chat.editQueuedMessage' }).click()

    await expect
      .element(screen.getByRole('textbox', { name: 'chat.inputAria' }))
      .toHaveValue('queued content')
    expect(draftChangeSpy.mock.calls.at(-1)?.[0]).toEqual(
      expect.objectContaining({
        message: 'queued content',
        queuedMessages: []
      })
    )
  })

  it('queues composer input instead of opening a new turn while a run is active', async () => {
    const screen = await render(<TestComposer isGenerating />)
    const input = screen.getByRole('textbox', { name: 'chat.inputAria' })

    await input.fill('wait for the current run')
    await input.click()
    await userEvent.keyboard('{Enter}')

    expect(submitSpy).not.toHaveBeenCalled()
    await expect.element(input).toHaveValue('')
    await expect.element(screen.getByText('wait for the current run')).toBeVisible()
    const latestDraft = draftChangeSpy.mock.calls.at(-1)?.[0] as ChatComposerProps['draft']
    expect(latestDraft.queuedMessages).toEqual([
      expect.objectContaining({
        content: 'wait for the current run',
        status: 'pending'
      })
    ])
  })

  it('reorders queued rows from the hover handle with the keyboard', async () => {
    const screen = await render(
      <TestComposer
        initialDraft={createComposerDraft({
          modelId: 'model-1',
          projectId: 'project-a',
          queuedMessages: [queuedMessage('one', '123', 1), queuedMessage('two', '345', 2)]
        })}
        isGenerating
      />
    )

    const firstHandle = screen.getByRole('button', { name: 'chat.reorderQueuedMessage' }).first()
    ;(firstHandle.element() as HTMLButtonElement).focus()
    await userEvent.keyboard('{ArrowDown}')

    expect(
      draftChangeSpy.mock.calls.at(-1)?.[0].queuedMessages.map((message) => message.id)
    ).toEqual(['two', 'one'])
  })

  it('animates rows out of the way while dragging and keeps the queue compact', async () => {
    const screen = await render(
      <TestComposer
        initialDraft={createComposerDraft({
          modelId: 'model-1',
          projectId: 'project-a',
          queuedMessages: [
            queuedMessage('one', '123', 1),
            queuedMessage('two', '345', 2),
            queuedMessage('three', '890', 3)
          ]
        })}
        isGenerating
      />
    )
    const rows = Array.from(screen.container.querySelectorAll<HTMLElement>('.guidance-queue__item'))
    const handles = Array.from(
      screen.container.querySelectorAll<HTMLButtonElement>('.guidance-queue__drag-handle')
    )
    expect(rows[0].getBoundingClientRect().height).toBeLessThanOrEqual(40)

    const dataTransfer = new DataTransfer()
    handles[0].dispatchEvent(
      new DragEvent('dragstart', { bubbles: true, cancelable: true, dataTransfer })
    )
    await expect.element(rows[0]).toHaveAttribute('data-dragging', 'true')
    rows[1].dispatchEvent(
      new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer })
    )

    await expect
      .poll(() => draftChangeSpy.mock.calls.at(-1)?.[0].queuedMessages.map((message) => message.id))
      .toEqual(['two', 'one', 'three'])
    await expect.element(rows[1]).toHaveAttribute('data-reordering', 'true')
    expect(rows[1].getAnimations().length).toBeGreaterThan(0)
    await expect
      .element(screen.getByText('345').element().closest<HTMLElement>('.guidance-queue__item'))
      .toHaveAttribute('data-drop-target', 'true')

    rows[0].dispatchEvent(
      new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer })
    )
    rows[1].dispatchEvent(
      new DragEvent('dragover', { bubbles: true, cancelable: true, dataTransfer })
    )
    await expect
      .poll(() => draftChangeSpy.mock.calls.at(-1)?.[0].queuedMessages.map((message) => message.id))
      .toEqual(['one', 'two', 'three'])

    rows[1].dispatchEvent(new DragEvent('drop', { bubbles: true, cancelable: true, dataTransfer }))
  })
})

describe('Composer permission confirmation', () => {
  it.each([false, true])(
    'keeps the current permission until full access is explicitly confirmed (running=%s)',
    async (isGenerating) => {
      const screen = await render(<TestComposer isGenerating={isGenerating} />)

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
    }
  )
})

describe('ChatComposer Skill picker', () => {
  it('lists and submits bundled and installed Skills without a project', async () => {
    const screen = await render(
      <TestComposer initialDraft={createComposerDraft({ modelId: 'model-1', projectId: null })} />
    )

    await screen.getByRole('button', { name: 'chat.addContext' }).click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()
    await expect.poll(() => listSkillsSpy.mock.calls.length).toBe(1)
    expect(listSkillsSpy).toHaveBeenCalledWith(null)
    await expect
      .element(screen.getByRole('button', { name: /^skills\.bundled\.documents\.name/ }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: /^Installed dependency auditor/ }))
      .toBeVisible()
    await expect
      .element(screen.getByRole('button', { name: /^skills\.bundled\.imageGeneration\.name/ }))
      .toBeVisible()
    expect(screen.container.textContent).not.toContain('chat.skillProjectRequired')

    await screen.getByRole('button', { name: /^skills\.bundled\.documents\.name/ }).click()
    await screen.getByRole('button', { name: /^Installed dependency auditor/ }).click()
    await screen.getByRole('textbox', { name: 'chat.inputAria' }).fill('Audit without a project')
    await screen.getByRole('button', { name: 'chat.send' }).click()
    await expect.poll(() => submitSpy.mock.calls.length).toBe(1)

    expect(submitSpy.mock.calls[0]?.[1]).toMatchObject({
      projectId: null,
      skills: [
        { id: bundledDocumentsSkill.id, revision: bundledDocumentsSkill.revision },
        { id: installedAuditorSkill.id, revision: installedAuditorSkill.revision }
      ]
    })
  })

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
    const diagnosticsSummary = screen.getByText('chat.skillDiagnostics', { exact: true })
    await expect.element(diagnosticsSummary).toBeVisible()
    await diagnosticsSummary.click()
    await expect
      .element(screen.getByText('skills.diagnosticsAvailable', { exact: true }))
      .toBeVisible()
    expect(screen.container.textContent).not.toContain('Broken frontmatter')
    await expect.element(screen.getByText('chat.noSkills', { exact: true })).toBeVisible()
  })

  it('shows catalog failures and lets the user retry', async () => {
    listSkillsSpy
      .mockRejectedValueOnce(new Error('catalog offline'))
      .mockResolvedValueOnce(catalog([]))
    const screen = await render(<TestComposer />)

    await screen.getByRole('button', { name: 'chat.addContext' }).click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()
    await expect.element(screen.getByRole('alert')).toHaveTextContent('chat.skillsLoadFailed')
    expect(screen.container.textContent).not.toContain('catalog offline')
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
      .toBe(2)
  })

  it('keeps source and trust metadata accessible without adding it to compact list rows', async () => {
    listSkillsSpy.mockResolvedValue(
      catalog([auditorSkill, testSkill, bundledDocumentsSkill, installedAuditorSkill])
    )
    const screen = await render(<TestComposer />)

    await screen.getByRole('button', { name: 'chat.addContext' }).click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()

    const bundledOption = screen.getByRole('button', {
      name: /^skills\.bundled\.documents\.name.*chat\.bundledSkill.*chat\.skillTrustApplication/
    })
    await expect.element(bundledOption).toBeVisible()
    expect(screen.container.textContent).not.toContain('chat.bundledSkill')
    expect(screen.container.textContent).not.toContain('chat.installedSkill')
    expect(screen.container.textContent).not.toContain('chat.workspaceSkill')
    ;(bundledOption.element() as HTMLButtonElement).focus()
    await expect
      .element(screen.getByRole('tooltip'))
      .toHaveTextContent('skills.bundled.documents.description')

    await bundledOption.click()
    await screen.getByRole('textbox', { name: 'chat.inputAria' }).fill('Audit this claim')
    await screen.getByRole('button', { name: 'chat.send' }).click()
    await expect.poll(() => submitSpy.mock.calls.length).toBe(1)

    expect(submitSpy.mock.calls[0]?.[1].skills).toEqual([
      { id: bundledDocumentsSkill.id, revision: bundledDocumentsSkill.revision }
    ])
  })

  it('uses dedicated Office icons for the three bundled Office skills', async () => {
    listSkillsSpy.mockResolvedValue(catalog(bundledOfficeSkills))
    const screen = await render(<TestComposer />)

    await screen.getByRole('button', { name: 'chat.addContext' }).click()
    await screen.getByRole('menuitem', { name: 'chat.skills' }).click()

    for (const [name, kind] of [
      ['skills.bundled.documents.name', 'document'],
      ['skills.bundled.spreadsheets.name', 'spreadsheet'],
      ['skills.bundled.presentations.name', 'presentation']
    ] as const) {
      const option = screen.getByRole('button', { name: new RegExp(`^${name}`) })
      await expect.element(option).toBeVisible()
      const icon = option.element().querySelector(`[data-office-kind="${kind}"]`)
      expect(icon?.querySelector('img')).not.toBeNull()
      expect(icon?.querySelector('svg')).toBeNull()
    }
  })

  it('keeps same-named selected skills visibly and accessibly distinct by provenance', async () => {
    const sameNamedBundledSkill = {
      ...bundledDocumentsSkill,
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
    expect(bundledChip?.textContent).toContain('skills.bundled.documents.name')
    expect(bundledChip?.querySelector('.composer-skill-chip__provenance')).toBeNull()
    expect(bundledChip?.textContent).not.toContain('chat.bundledSkill · chat.skillTrustApplication')
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
          name: 'chat.removeSkill skills.bundled.documents.name · chat.bundledSkill · chat.skillTrustApplication'
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

  it('drops workspace Skills but preserves global Skills when switching projects', async () => {
    const screen = await render(
      <TestComposer
        initialDraft={createComposerDraft({
          message: 'Inspect the other project',
          modelId: 'model-1',
          projectId: 'project-a',
          skills: [
            { id: auditorSkill.id, revision: auditorSkill.revision },
            { id: bundledDocumentsSkill.id, revision: bundledDocumentsSkill.revision }
          ]
        })}
        showProjectSelector
      />
    )

    await screen.getByRole('button', { name: /Project A/ }).click()
    await screen.getByRole('option', { name: /Project B/ }).click()
    await expect
      .poll(() => screen.container.querySelectorAll('.composer-skill-chip').length)
      .toBe(1)
    expect(screen.container.textContent).not.toContain(auditorSkill.name)
    expect(screen.container.textContent).toContain('skills.bundled.documents.name')

    await screen.getByRole('button', { name: 'chat.send' }).click()
    await expect.poll(() => submitSpy.mock.calls.length).toBe(1)
    expect(submitSpy.mock.calls[0]?.[1].projectId).toBe('project-b')
    expect(submitSpy.mock.calls[0]?.[1].skills).toEqual([
      { id: bundledDocumentsSkill.id, revision: bundledDocumentsSkill.revision }
    ])
  })
})
