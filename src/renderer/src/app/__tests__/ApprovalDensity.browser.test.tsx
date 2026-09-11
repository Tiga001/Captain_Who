import type { AgentProposedAction } from '@mycopilot/protocol'
import type { CSSProperties } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { ToastProvider } from '../../components/toast/ToastProvider'
import { getFrontendCssVariables } from '../../config/frontendConfig'
import { getFrontendTheme } from '../../config/frontendTheme'
import type { ChatConversation } from '../../features/chat/chatTypes'
import { AgentApprovalDialog } from '../../features/chat/components/AgentApprovalDialog'
import { ConversationSurface } from '../../features/chat/ConversationSurface'
import { createComposerDraft } from '../chatMessageFactory'
import '../../styles/global.css'

const fileChangeRpc = vi.hoisted(() => ({ getDiff: vi.fn() }))

vi.mock('../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../config/languageRegistry')
  const t = (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
  return {
    useFrontendConfig: () => ({
      language: 'zh-CN',
      t
    })
  }
})
// Approval replaces the visible Composer, which remains mounted to preserve its draft.
vi.mock('../../config/ModelSettingsProvider', () => ({
  useModelSettings: () => ({ enabledModels: [] })
}))
vi.mock('../../config/ProjectSettingsProvider', () => ({
  useProjectSettings: () => ({ projects: [], openCreateProjectDialog: vi.fn(async () => null) })
}))
vi.mock('../../features/chat/components/ImagePreview', () => ({
  useImagePreview: () => vi.fn(),
  useImagePreviewNotice: () => vi.fn()
}))
vi.mock('../../features/agent/agentClient', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../features/agent/agentClient')>()),
  getAgentFileChangeDiff: fileChangeRpc.getDiff
}))
vi.mock('../../host/hostClient', () => ({ hostClient: {} }))

const LONG_REASON =
  '检查当前项目中的文件修改与浏览器操作，保留完整的中文申请原因，并确保底部终端占据一半高度时仍然能够查看全部详情和选择审批操作。'
const LONG_PATH =
  'src/renderer/src/features/approval/用于验证审批卡片长文件路径自然换行的目录/entry-point.ts'
const MESSAGE_ID = 'assistant-approval-density'
const CALL_ID = `tc1_${'a'.repeat(43)}`
const DIGEST = 'e'.repeat(64)

function commandAction(long = true): Extract<AgentProposedAction, { type: 'command' }> {
  return {
    type: 'command',
    command: {
      id: 'command-density',
      command: long ? `pnpm exec prettier --check ${LONG_PATH}` : 'pnpm test',
      cwd: null,
      timeoutMs: null,
      approvalStatus: 'required',
      riskLevel: null,
      reason: long ? LONG_REASON : '运行测试',
      observe: null
    }
  }
}

function fileChangeAction(): Extract<AgentProposedAction, { type: 'file_change' }> {
  return {
    type: 'file_change',
    fileChange: {
      schemaVersion: 1,
      id: 'file-density',
      transactionId: 'transaction-density',
      operation: 'update',
      updateStrategy: null,
      filePath: LONG_PATH,
      inlineDiff: {
        patch:
          '@@ -1,12 +1,12 @@\n' +
          Array.from({ length: 12 }, (_, index) => ` line ${index + 1}\n`).join(''),
        truncated: false
      },
      baseRevision: 'content-sha256-v1:base',
      summary: LONG_REASON,
      additions: 0,
      deletions: 0,
      lineCount: 12,
      byteCount: 96,
      approvalStatus: 'required'
    }
  }
}

function capabilityAction(): Extract<
  AgentProposedAction,
  { type: 'builtin_capability_activation' }
> {
  const now = Math.floor(Date.now() / 1000)
  return {
    type: 'builtin_capability_activation',
    approval: {
      actionId: '94c2f39c-ddaa-49bb-a3ef-8756053d68c8',
      activationId: '67b4ea45-d0e2-42d0-aee4-6da42d5e45a7',
      runId: 'run-density',
      callId: CALL_ID,
      capabilityId: 'browser_automation',
      displayName: 'Browser automation',
      reason: LONG_REASON,
      manifestDigest: `sha256:${DIGEST}`,
      policyRevision: 7,
      createdAt: now,
      expiresAt: now + 900,
      approvalStatus: 'required'
    }
  }
}

function browserRiskAction(): Extract<AgentProposedAction, { type: 'browser_risk_approval' }> {
  const now = Math.floor(Date.now() / 1000)
  return {
    type: 'browser_risk_approval',
    approval: {
      schemaVersion: 1,
      actionId: '94c2f39c-ddaa-49bb-a3ef-8756053d68c8',
      riskApprovalId: 'a8a6102c-8ad6-45d5-bb0d-3e4f0ad2a30f',
      runId: 'run-density',
      callId: CALL_ID,
      capabilityId: 'browser_automation',
      capabilityActivationId: '67b4ea45-d0e2-42d0-aee4-6da42d5e45a7',
      displayName: 'Browser automation',
      reason: LONG_REASON,
      destination: {
        normalizedUrl: 'http://127.0.0.1:3000/approval-density/long-local-fixture-resource',
        origin: 'http://127.0.0.1:3000',
        scheme: 'http',
        asciiHost: '127.0.0.1',
        effectivePort: 3000,
        addressClass: 'loopback'
      },
      trigger: 'tool_argument',
      triggerToolName: 'browser_navigate',
      riskKinds: ['insecure_http', 'loopback', 'non_standard_port'],
      manifestDigest: `sha256:${DIGEST}`,
      policyRevision: 7,
      createdAt: now,
      expiresAt: now + 900,
      approvalStatus: 'required'
    }
  }
}

function builtinMcpAction(): Extract<AgentProposedAction, { type: 'builtin_mcp_tool_approval' }> {
  const now = Math.floor(Date.now() / 1000)
  return {
    type: 'builtin_mcp_tool_approval',
    approval: {
      schemaVersion: 1,
      identity: {
        actionId: '11111111-1111-4111-8111-111111111111',
        approvalId: '22222222-2222-4222-8222-222222222222',
        runId: 'run-density',
        callId: CALL_ID,
        capabilityId: 'browser_automation',
        capabilityActivationId: '33333333-3333-4333-8333-333333333333',
        managedMcpId: 'managed-browser',
        packageName: 'browser-fixture',
        packageVersion: '1.0.0',
        upstreamCatalogDigest: `sha256:${DIGEST}`,
        manifestDigest: `sha256:${DIGEST}`,
        policyDigest: `sha256:${DIGEST}`,
        policyRevision: 8,
        toolId: 'browser_evaluate',
        rawName: 'browser_evaluate',
        modelName: 'browser_evaluate',
        upstreamSchemaDigest: `sha256:${DIGEST}`,
        hostOverlayDigest: `sha256:${DIGEST}`,
        hostInputSchemaDigest: `sha256:${DIGEST}`,
        argumentsDigest: `sha256:${DIGEST}`,
        resourceScopeDigest: `sha256:${DIGEST}`,
        origin: 'https://fixture.example'
      },
      capabilityDisplayName: 'Browser automation',
      toolDisplayName: 'Evaluate script',
      callReason: LONG_REASON,
      operationCategory: 'page_script_execution',
      resourceSummary: {
        scope: 'managed_surface',
        displayName: 'Current managed browser tab',
        fileBasenames: ['需要完整显示的中文文件名称.txt'],
        origin: 'https://fixture.example'
      },
      riskKinds: ['page_script_execution'],
      createdAt: now,
      expiresAt: now + 900,
      approvalStatus: 'required'
    }
  }
}

function externalMcpAction(): Extract<AgentProposedAction, { type: 'mcp_tool_call' }> {
  const now = Date.now()
  return {
    type: 'mcp_tool_call',
    approval: {
      identity: {
        actionId: 'c2fd7f32-2ca4-4d34-a4c3-177d8fcb7457',
        invocationId: '5849f9ae-f7cb-4697-a56f-3622b823b08d',
        runId: 'run-density',
        callId: CALL_ID,
        provenance: {
          serverId: '3ac3ec2b-3549-4dca-bf7d-298511b42523',
          scope: { type: 'user' },
          rawToolName: 'echo_text',
          modelToolName: 'mcp_safe_echo',
          configEpoch: '44b009b5-adc5-4f85-8e52-b362528101cc',
          registryRevision: 7,
          configDigest: DIGEST,
          catalogGeneration: 4,
          catalogDigest: DIGEST,
          catalogSchemaDigest: DIGEST,
          schemaDigest: DIGEST,
          schemaNormalizerVersion: 1
        }
      },
      call: {
        id: CALL_ID,
        tool: 'mcp_safe_echo',
        args: {},
        approvalStatus: 'required',
        reason: null
      },
      summary: {
        serverId: '3ac3ec2b-3549-4dca-bf7d-298511b42523',
        serverDisplayName: '用于验证完整外部服务名称换行的测试服务器',
        scope: { type: 'user' },
        rawToolName: 'echo_text',
        modelToolName: 'mcp_safe_echo',
        displayReason: LONG_REASON,
        arguments: {
          encodedBytes: 2,
          topLevelPropertyCount: 0,
          stringValueCount: 0,
          numberValueCount: 0,
          booleanValueCount: 0,
          nullValueCount: 0,
          objectValueCount: 1,
          arrayValueCount: 0,
          maxDepth: 1,
          truncated: false
        },
        risk: 'read_only_claimed',
        external: true
      },
      approvalMode: 'prompt',
      payloadPersistence: 'process_only',
      createdAt: now,
      expiresAt: now + 900_000
    }
  }
}

function skillAction(): Extract<AgentProposedAction, { type: 'skill_installation' }> {
  return {
    type: 'skill_installation',
    installation: {
      schemaVersion: 1,
      id: 'install-density',
      installRef: `skill_install_${'a'.repeat(32)}`,
      approvalStatus: 'required',
      expiresAt: Date.now() + 900_000,
      preview: {
        name: '完整中文技能安装预览',
        description: LONG_REASON.repeat(3),
        sourceSummary: { kind: 'github', url: 'https://github.com/example/approval-density' },
        resolvedRevision: '0123456789abcdef',
        fileCount: 4,
        totalBytes: 2048,
        resourceSummary: { total: 3, references: 1, assets: 1, scripts: 1, bytes: 1024 },
        containsScripts: true,
        warnings: [
          { code: 'containsScripts', message: 'Contains scripts.', requiresAcknowledgement: true }
        ],
        compatibility: 'compatibleWithWarnings',
        operation: 'install',
        impact: 'addManagedSkill'
      }
    }
  }
}

const cases = [
  ['command', commandAction],
  ['file diff', fileChangeAction],
  ['built-in capability', capabilityAction],
  ['browser risk', browserRiskAction],
  ['built-in MCP', builtinMcpAction],
  ['external MCP', externalMcpAction],
  ['Skill installation', skillAction]
] as const

function conversation(action: AgentProposedAction): ChatConversation {
  return {
    id: 'approval-density',
    projectId: null,
    modelId: null,
    title: '审批卡片布局',
    createdAt: 1,
    updatedAt: 1,
    messages: [
      {
        id: 'user-density',
        role: 'user',
        content: '请检查当前项目。',
        createdAt: 1,
        status: 'sent'
      },
      {
        id: MESSAGE_ID,
        role: 'assistant',
        content: '',
        createdAt: 1,
        status: 'pending',
        agentRun: {
          runId: 'run-density',
          status: 'waiting_for_approval',
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [action],
          fileChangeProposals: [],
          timeline: []
        }
      }
    ]
  }
}

function Workspace({
  action,
  bottomOpen = true,
  onApprove,
  onReject
}: {
  action: AgentProposedAction
  bottomOpen?: boolean
  onApprove: () => Promise<boolean>
  onReject: () => Promise<boolean>
}) {
  return (
    <ToastProvider>
      <div
        className="app-shell"
        data-bottom-open={String(bottomOpen)}
        data-left-open="true"
        data-right-open="true"
        style={
          {
            width: 960,
            height: 760,
            '--left-panel-width': '280px',
            '--right-panel-width': '360px',
            '--bottom-panel-height': bottomOpen ? '380px' : '0px'
          } as CSSProperties
        }
      >
        <aside className="side-panel side-panel--left">项目</aside>
        <main className="main-panel">
          <div className="main-panel__toolbar" data-testid="toolbar" />
          <div className="main-panel__surface">
            <ConversationSurface
              conversation={conversation(action)}
              composerDraft={createComposerDraft()}
              editSelectedModelAvailable={false}
              editSelectedModelSupportsImage={false}
              initialScrollTop={0}
              mode="interactive"
              onApproveAgentAction={onApprove}
              onComposerDraftChange={vi.fn()}
              onRejectAgentAction={onReject}
              onSubmitMessage={vi.fn()}
              permissionModeAvailability={{ custom: true, full: true }}
              showTokenUsageDetails={false}
            />
          </div>
        </main>
        <aside className="side-panel side-panel--right">审查</aside>
        <aside className="side-panel side-panel--bottom">终端区域</aside>
      </div>
    </ToastProvider>
  )
}

function requiredElement<T extends HTMLElement>(root: ParentNode, selector: string): T {
  const element = root.querySelector<T>(selector)
  if (!element) throw new Error(`Missing approval fixture element: ${selector}`)
  return element
}

function expectNoHorizontalOverflow(element: HTMLElement) {
  expect(element.scrollWidth).toBeLessThanOrEqual(element.clientWidth + 1)
}

function expectReachable(element: HTMLElement, viewport: HTMLElement) {
  const bounds = element.getBoundingClientRect()
  const visible = viewport.getBoundingClientRect()
  expect(bounds.top).toBeGreaterThanOrEqual(visible.top - 1)
  expect(bounds.bottom).toBeLessThanOrEqual(visible.bottom + 1)
  expect(bounds.left).toBeGreaterThanOrEqual(visible.left)
  expect(bounds.right).toBeLessThanOrEqual(visible.right)
  expect(
    element.contains(
      document.elementFromPoint(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2)
    )
  ).toBe(true)
}

let previousRootStyle: string | null
beforeEach(async () => {
  previousRootStyle = document.documentElement.getAttribute('style')
  const theme = getFrontendTheme('classic-light')
  for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens))) {
    document.documentElement.style.setProperty(key, value)
  }
  fileChangeRpc.getDiff.mockReset()
  await page.viewport(1120, 820)
})
afterEach(async () => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
  await page.viewport(1280, 720)
})

describe('Approval density in the real conversation surface', () => {
  it.each(cases)(
    'keeps %s approvals reachable in a 320px chat column above a half-height terminal',
    async (_name, factory) => {
      const action = factory()
      const onApprove = vi.fn<() => Promise<boolean>>().mockResolvedValue(false)
      const onReject = vi.fn<() => Promise<boolean>>().mockResolvedValue(false)
      const screen = await render(
        <Workspace action={action} onApprove={onApprove} onReject={onReject} />
      )
      const surface = requiredElement(screen.container, '.conversation-surface')
      const composer = requiredElement(surface, '.chat-conversation-page__composer')
      const dialog = requiredElement(composer, '.agent-approval-dialog')
      const messages = requiredElement(surface, '.chat-conversation-page__messages-region')
      const bottom = requiredElement(screen.container, '.side-panel--bottom')
      const toolbar = requiredElement(screen.container, '[data-testid="toolbar"]')
      const toolbarTop = toolbar.getBoundingClientRect().top
      const bottomTop = bottom.getBoundingClientRect().top
      const primary = requiredElement<HTMLButtonElement>(dialog, '[data-choice="primary"]')
      const reject = requiredElement<HTMLButtonElement>(dialog, '.agent-approval-dialog__reject')

      expect(surface.getBoundingClientRect().width).toBe(320)
      expect(bottom.getBoundingClientRect().height).toBe(380)
      expect(messages.getBoundingClientRect().height).toBeGreaterThanOrEqual(64)
      expect(surface.getBoundingClientRect().bottom).toBeLessThanOrEqual(bottomTop)
      expect(getComputedStyle(composer).paddingLeft).toBe('16px')
      expect(getComputedStyle(composer).paddingRight).toBe('16px')
      expect(getComputedStyle(composer).overflowY).toBe('auto')
      expect(getComputedStyle(dialog).minHeight).toBe('0px')
      expect(getComputedStyle(dialog).padding).toBe('12px 12px 10px')
      expect(getComputedStyle(dialog).gap).toBe('6px')
      expect(getComputedStyle(dialog).borderRadius).toBe('16px')
      expect(getComputedStyle(dialog).fontSize).toBe('13px')
      expect(primary.getBoundingClientRect().height).toBeGreaterThanOrEqual(32)
      expect(reject.getBoundingClientRect().height).toBe(28)
      expectNoHorizontalOverflow(surface)
      expectNoHorizontalOverflow(composer)
      expectNoHorizontalOverflow(dialog)

      const details = dialog.querySelector<HTMLElement>('.agent-approval-dialog__details')
      if (details) {
        expect(details.getBoundingClientRect().height).toBeLessThanOrEqual(180)
        expect(getComputedStyle(details).overflowY).toBe('auto')
        expectNoHorizontalOverflow(details)
      }
      const definitionList = details?.querySelector<HTMLElement>('dl')
      if (definitionList) {
        expect(getComputedStyle(definitionList).display).toBe('grid')
        expect(getComputedStyle(definitionList).fontSize).toBe('13px')
        expect(getComputedStyle(definitionList).gap).toBe('6px')
        for (const value of definitionList.querySelectorAll<HTMLElement>('dd')) {
          expect(getComputedStyle(value).marginLeft).toBe('0px')
          expectNoHorizontalOverflow(value)
        }
        if (details && details.scrollHeight > details.clientHeight) {
          const dialogHeight = dialog.getBoundingClientRect().height
          details.scrollTop = details.scrollHeight
          expect(details.scrollTop).toBeGreaterThan(0)
          const lastValue = requiredElement(definitionList, ':scope > div:last-child dd')
          expect(lastValue.getBoundingClientRect().bottom).toBeLessThanOrEqual(
            details.getBoundingClientRect().bottom
          )
          expect(dialog.getBoundingClientRect().height).toBe(dialogHeight)
        }
      }

      composer.scrollTop = composer.scrollHeight
      expectReachable(primary, composer)
      await userEvent.click(primary)
      expect(onApprove).toHaveBeenCalledExactlyOnceWith(MESSAGE_ID, action, 'singleAction')
      await expect.element(reject).toBeEnabled()
      const reason = requiredElement<HTMLInputElement>(
        dialog,
        '.agent-approval-dialog__reject-row input'
      )
      await userEvent.fill(reason, '请缩小修改范围')
      composer.scrollTop = composer.scrollHeight
      expectReachable(reject, composer)
      expect(reject.getBoundingClientRect().bottom).toBeLessThan(bottomTop)
      await userEvent.click(reject)
      expect(onReject).toHaveBeenCalledExactlyOnceWith(MESSAGE_ID, action, '请缩小修改范围')
      expect(toolbar.getBoundingClientRect().top).toBe(toolbarTop)
      expect(bottom.getBoundingClientRect().top).toBe(bottomTop)
      expect(fileChangeRpc.getDiff).not.toHaveBeenCalled()
    }
  )

  it('removes the old minimum-height gap from a short command without shrinking its choices', async () => {
    const screen = await render(
      <div style={{ width: 440 }}>
        <AgentApprovalDialog target={{ action: commandAction(false), messageId: MESSAGE_ID }} />
      </div>
    )
    const dialog = requiredElement(screen.container, '.agent-approval-dialog')
    const title = requiredElement(dialog, '.agent-approval-dialog__request')
    const choice = requiredElement(dialog, '[data-choice="primary"]')
    const index = requiredElement(choice, '.agent-approval-dialog__index')
    expect(dialog.getBoundingClientRect().height).toBeLessThan(194)
    const style = getComputedStyle(dialog)
    const contentHeight = [...dialog.children].reduce(
      (height, child) => height + child.getBoundingClientRect().height,
      0
    )
    const expectedHeight =
      contentHeight +
      (dialog.children.length - 1) * parseFloat(style.rowGap) +
      parseFloat(style.paddingTop) +
      parseFloat(style.paddingBottom) +
      parseFloat(style.borderTopWidth) +
      parseFloat(style.borderBottomWidth)
    expect(dialog.getBoundingClientRect().height).toBeCloseTo(expectedHeight, 1)
    expect(getComputedStyle(title).fontSize).toBe('14px')
    expect(getComputedStyle(title).lineHeight).toBe('19.6px')
    expect(choice.getBoundingClientRect().height).toBe(32)
    expect(index.getBoundingClientRect().width).toBe(18)
    expect(index.getBoundingClientRect().height).toBe(18)
  })

  it('wraps Chinese remember text and long paths while preserving the five-row diff viewport', async () => {
    const action = fileChangeAction()
    const onApprove = vi.fn<() => Promise<boolean>>().mockResolvedValue(false)
    const screen = await render(
      <Workspace action={action} bottomOpen={false} onApprove={onApprove} onReject={vi.fn()} />
    )
    const composer = requiredElement(screen.container, '.chat-conversation-page__composer')
    const dialog = requiredElement(composer, '.agent-approval-dialog')
    const title = requiredElement(dialog, '.agent-approval-dialog__request')
    const path = requiredElement(dialog, '.agent-approval-dialog__command')
    const remember = requiredElement<HTMLButtonElement>(dialog, '[data-choice="remember"]')
    const label = requiredElement(remember, '.agent-approval-dialog__choice-text')
    const diff = requiredElement(dialog, '.agent-approval-dialog__file-change-renderer')

    expect(title.textContent).toBe(LONG_REASON)
    expect(title.getBoundingClientRect().height).toBeGreaterThan(
      2 * parseFloat(getComputedStyle(title).lineHeight)
    )
    expect(path.textContent).toBe(LONG_PATH)
    expect(path.getBoundingClientRect().height).toBeGreaterThan(
      2 * parseFloat(getComputedStyle(path).lineHeight)
    )
    expectNoHorizontalOverflow(path)
    expect(label.getBoundingClientRect().height).toBeGreaterThan(
      parseFloat(getComputedStyle(label).lineHeight)
    )
    expectNoHorizontalOverflow(remember)
    expect(diff.getBoundingClientRect().height).toBe(95)
    expect(getComputedStyle(diff).overflowY).toBe('auto')
    expect(diff.scrollHeight).toBeGreaterThan(diff.clientHeight)
    diff.scrollTop = diff.scrollHeight
    expect(diff.scrollTop).toBeGreaterThan(0)
    expect(composer.scrollHeight).toBeLessThanOrEqual(composer.clientHeight + 1)
    expectReachable(remember, composer)
    await userEvent.click(remember)
    expect(onApprove).toHaveBeenCalledExactlyOnceWith(
      MESSAGE_ID,
      action,
      'remainingApplyPatchInRun'
    )
    expect(fileChangeRpc.getDiff).not.toHaveBeenCalled()
  })

  it('scrolls long command text independently and restores composer space when the terminal closes', async () => {
    const action = commandAction()
    action.command.command = Array.from(
      { length: 35 },
      (_, index) => `echo '第${index + 1}行检查命令'`
    ).join('\n')
    const onApprove = vi.fn<() => Promise<boolean>>().mockResolvedValue(false)
    const onReject = vi.fn<() => Promise<boolean>>().mockResolvedValue(false)
    const screen = await render(
      <Workspace action={action} onApprove={onApprove} onReject={onReject} />
    )
    const composer = requiredElement(screen.container, '.chat-conversation-page__composer')
    const command = requiredElement(composer, '.agent-approval-dialog__command')
    const reject = requiredElement(composer, '.agent-approval-dialog__reject')
    const initialRejectTop = reject.getBoundingClientRect().top
    expect(command.textContent).toBe(action.command.command)
    expect(command.getBoundingClientRect().height).toBeLessThanOrEqual(144)
    expect(command.scrollHeight).toBeGreaterThan(command.clientHeight)
    expect(composer.scrollHeight).toBeGreaterThan(composer.clientHeight)
    command.scrollTop = command.scrollHeight
    expect(command.scrollTop).toBeGreaterThan(0)
    expect(reject.getBoundingClientRect().top).toBe(initialRejectTop)
    composer.scrollTop = composer.scrollHeight
    expectReachable(reject, composer)

    await screen.rerender(
      <Workspace action={action} bottomOpen={false} onApprove={onApprove} onReject={onReject} />
    )
    await expect.poll(() => composer.scrollHeight - composer.clientHeight).toBeLessThanOrEqual(1)
    expectReachable(reject, composer)
    expect(command.textContent).toBe(action.command.command)
    expect(onApprove).not.toHaveBeenCalled()
    expect(onReject).not.toHaveBeenCalled()
  })
})
