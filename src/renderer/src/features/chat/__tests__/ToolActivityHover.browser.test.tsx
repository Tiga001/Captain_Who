import { useState, type CSSProperties } from 'react'
import { FileText, Send, SquareTerminal } from 'lucide-react'
import { expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { frontendConfig, getFrontendCssVariables } from '../../../config/frontendConfig'
import { getTranslation, type TranslationKey } from '../../../config/frontendTranslations'
import { classicDarkTheme, classicLightTheme } from '../../../config/themes/classic'
import { ensureAgentRun } from '../../agentRun/agentEventReducer'
import { AgentActivityDisclosure } from '../components/toolActivities/AgentActivityDisclosure'
import { BasicToolActivityHeader } from '../components/toolActivities/BasicToolActivityHeader'
import { FileChangeRow } from '../components/toolActivities/FileChangeToolActivity'
import { SearchToolActivityGroup } from '../components/toolActivities/SearchToolActivity'
import '../ChatConversationPage.agent.css'
import '../ChatConversationPage.results.css'
import '../../../styles/global.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: TranslationKey) => getTranslation('zh-CN', key) })
}))
vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))
vi.mock('../../storage/storageClient', () => ({ revealStoredProjectFile: vi.fn() }))
vi.mock('../components/toolActivities/FileChangeDiffCard', () => ({
  FileChangeDiffCard: () => <div>文件修改明细</div>
}))

function Fixture({ dark }: { dark: boolean }) {
  const [expanded, setExpanded] = useState(false)
  const run = ensureAgentRun(undefined, 'run', 'completed')
  run.toolCalls = ['read_file', 'run_command'].map((tool) => ({
    id: tool,
    tool,
    args: {},
    approvalStatus: 'not_required',
    reason: null
  }))
  run.toolResults = run.toolCalls.map((call) => ({ callId: call.id, tool: call.tool, ok: true }))
  return (
    <div
      data-testid="fixture"
      style={
        {
          ...getFrontendCssVariables(frontendConfig, dark ? classicDarkTheme : classicLightTheme),
          width: 760,
          padding: 24,
          boxSizing: 'border-box',
          background: 'var(--mc-color-surface-main-panel)',
          color: 'var(--mc-color-text-primary)',
          fontFamily: 'var(--mc-font-family)'
        } as CSSProperties
      }
    >
      <button data-testid="away" style={{ marginBottom: 24 }}>
        测试焦点
      </button>
      <div className="agent-run">
        <BasicToolActivityHeader
          run={run}
          items={[
            {
              id: 'read',
              callIds: ['read_file'],
              firstOrdinal: 0,
              latestOrdinal: 0,
              category: 'read'
            },
            {
              id: 'command',
              callIds: ['run_command'],
              firstOrdinal: 1,
              latestOrdinal: 1,
              category: 'command'
            }
          ]}
          expanded={expanded}
          controls="aggregate-detail"
          onToggle={() => setExpanded((value) => !value)}
        />
        <div hidden={!expanded} id="aggregate-detail">
          聚合操作明细
        </div>
        <AgentActivityDisclosure icon={FileText} label="已读取 4 个文件" hasDetails>
          <div className="agent-activity__details">读取明细</div>
        </AgentActivityDisclosure>
        <AgentActivityDisclosure
          className="agent-activity--send-message-group"
          icon={Send}
          label="已发送 3 条消息"
          hasDetails
        >
          <div className="agent-activity__details">消息明细</div>
        </AgentActivityDisclosure>
        <div className="basic-tool-activity__leaf" data-grouped="true">
          <AgentActivityDisclosure icon={SquareTerminal} label="已运行命令 npm test" hasDetails>
            <div className="agent-activity__details">命令输出</div>
          </AgentActivityDisclosure>
        </div>
        <div className="agent-activity agent-activity--basic-file-change">
          <FileChangeRow
            projectId="project"
            showIcon
            item={{
              call: {
                id: 'edit',
                tool: 'apply_patch',
                approvalStatus: 'not_required',
                reason: null,
                args: {
                  request: {
                    action: 'apply',
                    operation: 'update',
                    filePath: 'reports/derivation.md',
                    content: 'new content'
                  }
                }
              },
              preview: {
                schemaVersion: 1,
                previewId: 'preview',
                streamId: 'stream',
                attempt: 1,
                toolCallIndex: 0,
                toolCallId: 'edit',
                transactionId: 'transaction',
                filePath: 'reports/derivation.md',
                additions: 105,
                deletions: 101,
                lineCount: 109,
                byteCount: 128,
                generatedBytes: 128,
                updatedAt: 1,
                receivedAt: 1,
                content: 'new content'
              }
            }}
          />
        </div>
        <SearchToolActivityGroup
          kind="code"
          items={['TODO', 'FIXME'].map((query) => ({
            call: {
              id: query,
              tool: 'search_code',
              args: { query },
              approvalStatus: 'not_required',
              reason: null
            },
            result: { callId: query, tool: 'search_code', ok: true, result: { query, matches: [] } }
          }))}
        />
      </div>
    </div>
  )
}

it.each([false, true])(
  'uses transparent row hover and transient arrows in theme dark=%s',
  async (dark) => {
    await page.viewport(900, 900)
    const screen = await render(<Fixture dark={dark} />)
    const away = screen.container.querySelector<HTMLElement>('[data-testid="away"]')!
    const panel = screen.container.querySelector<HTMLElement>('[data-testid="fixture"]')!
    const fileLinkColor = getComputedStyle(
      panel.querySelector('.file-change-activity__path')!
    ).color
    const controls = [
      ...panel.querySelectorAll<HTMLElement>(
        '.basic-tool-activity__header, .agent-activity > summary'
      ),
      panel.querySelector<HTMLElement>('.file-change-activity__toggle')!
    ]
    for (const control of controls) {
      const file = control.classList.contains('file-change-activity__toggle')
      const row = file ? control.closest<HTMLElement>('.file-change-activity__item-line')! : control
      const arrow = file
        ? control
        : control.querySelector<HTMLElement>(
            '.basic-tool-activity__chevron, .agent-activity__chevron'
          )!
      await userEvent.click(away)
      await vi.waitFor(() => expect(getComputedStyle(arrow).opacity).toBe('0'))
      const rect = row.getBoundingClientRect()
      const normalColor = getComputedStyle(row).color
      const countColors = [...row.querySelectorAll('.rolling-line-count')].map(
        (item) => getComputedStyle(item).color
      )
      await userEvent.hover(row)
      await vi.waitFor(() => {
        expect(getComputedStyle(arrow).opacity).toBe('1')
        expect(getComputedStyle(row).color).not.toBe(normalColor)
      })
      expect(getComputedStyle(control).backgroundColor).toBe('rgba(0, 0, 0, 0)')
      expect(row.getBoundingClientRect().width).toBe(rect.width)
      expect(
        [...row.querySelectorAll('.rolling-line-count')].map((item) => getComputedStyle(item).color)
      ).toEqual(countColors)
      await userEvent.click(control)
      expect(
        control.getAttribute('aria-expanded') ??
          String((control.parentElement as HTMLDetailsElement).open)
      ).toBe('true')
      await userEvent.hover(away)
      await vi.waitFor(() => expect(getComputedStyle(arrow).opacity).toBe('0'))
      // Pointer focus does not keep the arrow visible; genuine keyboard focus does.
      await userEvent.keyboard('{Tab}')
      control.focus()
      await vi.waitFor(() => expect(getComputedStyle(arrow).opacity).toBe('1'))
      expect(getComputedStyle(control).backgroundColor).toBe('rgba(0, 0, 0, 0)')
    }

    const query = panel.querySelector<HTMLElement>('.search-activity__query-group > summary')!
    const queryArrow = query.querySelector('.search-activity__query-chevron')!
    await userEvent.click(away)
    expect(getComputedStyle(queryArrow).opacity).toBe('0')
    await userEvent.hover(query)
    await vi.waitFor(() => expect(getComputedStyle(queryArrow).opacity).toBe('1'))
    await userEvent.click(query)
    await userEvent.hover(away)
    await vi.waitFor(() => expect(getComputedStyle(queryArrow).opacity).toBe('0'))

    await page.screenshot({
      element: panel,
      path: `../../../../../../.cache/tool-hover/${dark ? 'dark' : 'light'}-rest.png`
    })
    const fileRow = panel.querySelector<HTMLElement>('.file-change-activity__item-line')!
    await userEvent.hover(fileRow)
    await vi.waitFor(() =>
      expect(
        getComputedStyle(fileRow.querySelector('.file-change-activity__toggle')!).opacity
      ).toBe('1')
    )
    expect(getComputedStyle(fileRow.querySelector('.file-change-activity__path')!).color).toBe(
      fileLinkColor
    )
    await page.screenshot({
      element: panel,
      path: `../../../../../../.cache/tool-hover/${dark ? 'dark' : 'light'}-hover.png`
    })
  }
)
