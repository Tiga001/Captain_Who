import type { AgentToolCall, AgentToolIdentity } from '@mycopilot/protocol'
import { describe, expect, it, vi } from 'vitest'
import type { ChatAgentRunView, ChatAgentTimelineItem } from '../chatTypes'
import {
  getBasicToolRepresentative,
  projectBasicToolTimeline,
  type BasicToolCategory
} from '../components/basicToolTimeline'

vi.mock('../../../host/hostClient', () => ({ hostClient: {} }))

function call(id: string, tool: AgentToolCall['tool'], args: unknown = {}): AgentToolCall {
  return { id, tool, args, approvalStatus: 'not_required', reason: null }
}

function marker(
  toolCall: AgentToolCall,
  identity: AgentToolIdentity | undefined = { type: 'builtin', toolName: toolCall.tool }
): ChatAgentTimelineItem {
  return { id: `marker-${toolCall.id}`, type: 'tool_call', callId: toolCall.id, identity }
}

function run(
  toolCalls: AgentToolCall[],
  timeline = toolCalls.map((entry) => marker(entry))
): ChatAgentRunView {
  return {
    runId: 'basic-tools-run',
    status: 'running',
    toolDefinitions: [],
    toolCalls,
    toolResults: [],
    approvals: [],
    fileChangeProposals: [],
    timeline
  }
}

function basicBlocks(current: ChatAgentRunView, timeline = current.timeline) {
  return projectBasicToolTimeline(current, timeline).filter((block) => block.kind === 'basic_tools')
}

describe('basic tool timeline projection', () => {
  it('allows the twelve typed builtins and preserves mixed raw marker order', () => {
    const tools: [AgentToolCall['tool'], BasicToolCategory][] = [
      ['read_file', 'read'],
      ['read_word', 'read'],
      ['read_presentation', 'read'],
      ['read_spreadsheet', 'read'],
      ['workspace_map', 'workspace'],
      ['search_files', 'search'],
      ['search_code', 'search'],
      ['attachments_list', 'attachments'],
      ['attachments_list_project', 'attachments'],
      ['conversation_history', 'history'],
      ['apply_patch', 'edit'],
      ['run_command', 'command']
    ]
    const current = run(tools.map(([tool], index) => call(String(index), tool)))
    const snapshot = structuredClone(current)
    const blocks = projectBasicToolTimeline(current, current.timeline)
    expect(blocks).toEqual([
      {
        kind: 'basic_tools',
        id: 'basic-tools-marker-0',
        items: tools.map(([, category], index) => ({
          id: `marker-${index}`,
          callIds: [String(index)],
          category,
          firstOrdinal: index,
          latestOrdinal: index
        }))
      }
    ])
    expect(current).toEqual(snapshot)
  })

  it.each<AgentToolIdentity | undefined>([
    undefined,
    { type: 'builtin', toolName: 'read_file' },
    { type: 'unregistered', toolName: 'run_command' },
    { type: 'runtime_extension', toolName: 'run_command', extensionId: 'external' }
  ])('keeps missing or nonmatching builtin provenance outside basic spans: %j', (identity) => {
    const calls = [
      call('before', 'read_file'),
      call('middle', 'run_command'),
      call('after', 'read_file')
    ]
    const timeline = calls.map((entry) => marker(entry))
    timeline[1] = { ...marker(calls[1]), identity } as ChatAgentTimelineItem
    const current = run(calls, timeline)
    expect(projectBasicToolTimeline(current, timeline).map((block) => block.kind)).toEqual([
      'basic_tools',
      'legacy',
      'basic_tools'
    ])
    expect(basicBlocks(current).flatMap((block) => block.items.map((item) => item.id))).toEqual([
      'marker-before',
      'marker-after'
    ])
  })

  it.each<AgentToolCall['tool']>([
    'read_image',
    'web_search',
    'web_fetch',
    'office_presentation',
    'send_message',
    'todo_update',
    'request_user_input'
  ])('leaves the excluded builtin %s as a visible boundary', (tool) => {
    const current = run([
      call('before', 'read_file'),
      call('special', tool),
      call('after', 'run_command')
    ])
    expect(projectBasicToolTimeline(current, current.timeline).map((block) => block.kind)).toEqual([
      'basic_tools',
      'legacy',
      'basic_tools'
    ])
  })

  it('ignores hidden bookkeeping and blank narration without changing raw ordinals', () => {
    const calls = [
      call('read', 'read_file'),
      call('wait', 'wait_agent'),
      call('command', 'run_command')
    ]
    const current = run(calls, [
      marker(calls[0]),
      { id: 'blank', type: 'message', content: ' \n ' },
      marker(calls[1]),
      marker(calls[2])
    ])
    expect(basicBlocks(current)[0].items.map((item) => [item.id, item.firstOrdinal])).toEqual([
      ['marker-read', 0],
      ['marker-command', 3]
    ])
    expect(projectBasicToolTimeline(current, current.timeline)).toHaveLength(1)
  })

  it('keeps the exact Host Skill activation hidden without splitting basic activity', () => {
    const calls = [
      call('before', 'read_file'),
      call('activation', 'skills_activate', { privateResource: 'not-a-timeline-row' }),
      call('after', 'run_command')
    ]
    const activation = marker(calls[1], {
      type: 'runtime_extension',
      extensionId: 'skills',
      toolName: 'skills_activate'
    })
    const current = run(calls, [marker(calls[0]), activation, marker(calls[2])])
    const blocks = projectBasicToolTimeline(current, current.timeline)
    expect(blocks).toHaveLength(1)
    expect(basicBlocks(current)[0].items.map((item) => item.callIds)).toEqual([
      ['before'],
      ['after']
    ])
  })

  it('preserves existing homogeneous Web grouping for the exact web.search extension', () => {
    const calls = [
      call('search-one', 'web_search'),
      call('search-two', 'web_search'),
      call('fetch-one', 'web_fetch'),
      call('fetch-two', 'web_fetch')
    ]
    const timeline = calls.map((entry) =>
      marker(entry, { type: 'runtime_extension', extensionId: 'web.search', toolName: entry.tool })
    )
    const current = run(calls, timeline)
    expect(projectBasicToolTimeline(current, timeline)).toEqual([
      {
        kind: 'legacy',
        items: [
          {
            id: 'web-search-group-search-one',
            type: 'web_activity_group',
            kind: 'search',
            callIds: ['search-one', 'search-two']
          },
          {
            id: 'web-fetch-group-fetch-one',
            type: 'web_activity_group',
            kind: 'fetch',
            callIds: ['fetch-one', 'fetch-two']
          }
        ]
      }
    ])
  })

  it.each<AgentToolCall['tool']>(['skills_activate', 'web_search'])(
    'does not grant the %s legacy route to an unrelated extension owner',
    (tool) => {
      const calls = [call('first', tool), call('second', tool)]
      const timeline = calls.map((entry) =>
        marker(entry, { type: 'runtime_extension', extensionId: 'external', toolName: entry.tool })
      )
      const current = run(calls, timeline)
      expect(projectBasicToolTimeline(current, timeline)).toEqual(
        timeline.map((entry) => ({ kind: 'legacy', items: [entry] }))
      )
    }
  )

  it.each<AgentToolCall['tool']>(['run_command', 'command_session'])(
    'preserves MCP provenance even when its %s call claims a matching builtin identity',
    (tool) => {
      const calls = [
        call('before', 'read_file'),
        call('external', tool),
        call('after', 'run_command')
      ]
      const current = run(calls)
      current.mcpInvocations = [
        {
          actionId: 'external-action',
          invocationId: 'external-invocation',
          callId: 'external',
          serverId: 'external-server',
          serverDisplayName: 'External',
          rawToolName: tool,
          modelToolName: tool,
          external: true,
          state: 'completed',
          dispatchCertainty: 'response_received',
          outcome: 'succeeded',
          outputTruncated: false
        }
      ]
      const blocks = projectBasicToolTimeline(current, current.timeline)
      expect(blocks.map((block) => block.kind)).toEqual(['basic_tools', 'legacy', 'basic_tools'])
      expect(blocks[1]).toEqual({ kind: 'legacy', items: [current.timeline[1]] })
    }
  )

  it('does not hide a foreign extension whose tool is named command_session', () => {
    const calls = [
      call('before', 'read_file'),
      call('foreign', 'command_session'),
      call('after', 'run_command')
    ]
    const foreign = marker(calls[1], {
      type: 'runtime_extension',
      extensionId: 'external',
      toolName: 'command_session'
    })
    const current = run(calls, [marker(calls[0]), foreign, marker(calls[2])])
    const blocks = projectBasicToolTimeline(current, current.timeline)
    expect(blocks.map((block) => block.kind)).toEqual(['basic_tools', 'legacy', 'basic_tools'])
    expect(blocks[1]).toEqual({ kind: 'legacy', items: [foreign] })
  })

  const visibleBoundaries: ChatAgentTimelineItem[] = [
    { id: 'narration', type: 'message', content: 'Continue here.' },
    { id: 'missing', type: 'tool_call', callId: 'unknown' },
    { id: 'mcp', type: 'mcp_tool_call', invocationId: 'external' },
    { id: 'error', type: 'error', message: 'Failure' },
    { id: 'compaction', type: 'context_compaction', operationId: 'compact', status: 'running' },
    {
      id: 'guidance',
      type: 'user_guidance',
      clientMessageId: 'human',
      content: 'Change course.',
      attachments: [],
      status: 'applied',
      createdAt: 1
    },
    {
      id: 'mail',
      type: 'workflow_delivery',
      inputId: 'input',
      deliveryId: 'delivery',
      instanceId: 'workflow',
      workflowName: 'Review',
      content: 'Feedback',
      createdAt: 1,
      traceSequence: 2,
      sources: []
    }
  ]

  it.each(visibleBoundaries)(
    'respects the $type boundary before deduplicating edits',
    (boundary) => {
      const args = { request: { action: 'append', transactionId: 'transaction' } }
      const calls = [call('first', 'apply_patch', args), call('last', 'apply_patch', args)]
      const current = run(calls, [marker(calls[0]), boundary, marker(calls[1])])
      const blocks = projectBasicToolTimeline(current, current.timeline)
      expect(blocks.map((block) => block.kind)).toEqual(['basic_tools', 'legacy', 'basic_tools'])
      expect(basicBlocks(current).map((block) => block.items[0].callIds)).toEqual([
        ['first'],
        ['last']
      ])
    }
  )

  it('deduplicates staged edits in first-occurrence order while tracking the latest operation ordinal', () => {
    const args = (action: string) => ({ request: { action, transactionId: 'transaction' } })
    const calls = [
      call('begin', 'apply_patch', { request: { action: 'begin', filePath: 'report.md' } }),
      call('read', 'read_file'),
      call('append', 'apply_patch', args('append')),
      call('command', 'run_command'),
      call('commit', 'apply_patch', args('commit'))
    ]
    const current = run(calls)
    current.toolResults = [
      { callId: 'begin', tool: 'apply_patch', ok: true, result: { transactionId: 'transaction' } }
    ]
    const items = basicBlocks(current)[0].items
    expect(
      items.map((item) => [item.id, item.callIds, item.firstOrdinal, item.latestOrdinal])
    ).toEqual([
      ['marker-begin', ['begin', 'append', 'commit'], 0, 4],
      ['marker-read', ['read'], 1, 1],
      ['marker-command', ['command'], 3, 3]
    ])
    expect(getBasicToolRepresentative(current, items[0])).toEqual({
      call: calls[4],
      result: undefined
    })
  })

  it('retains the first marker identity when a direct edit learns its transaction and when the span grows', () => {
    const first = call('direct', 'apply_patch', {
      request: { action: 'apply', filePath: 'report.md' }
    })
    const initial = run([first])
    const before = basicBlocks(initial)[0]
    const later = run([first, call('next', 'run_command')])
    later.toolResults = [
      { callId: first.id, tool: first.tool, ok: true, result: { transactionId: 'transaction' } }
    ]
    const after = basicBlocks(later)[0]
    expect(after.id).toBe(before.id)
    expect(after.items[0].id).toBe(before.items[0].id)
    expect(after.items[0]).toEqual(before.items[0])
  })

  it('preserves legacy category grouping without injecting a synthetic skill-load group', () => {
    const calls = [call('legacy-one', 'run_command'), call('legacy-two', 'run_command')]
    const timeline: ChatAgentTimelineItem[] = calls.map((entry) => ({
      id: `marker-${entry.id}`,
      type: 'tool_call',
      callId: entry.id
    }))
    const current = run(calls, timeline)
    current.activatedSkills = [
      {
        id: 'bundled:application:spreadsheets',
        name: 'Spreadsheets',
        revision: 'revision-1',
        source: { kind: 'bundled', id: 'application:spreadsheets' }
      }
    ]
    expect(projectBasicToolTimeline(current, timeline)).toEqual([
      {
        kind: 'legacy',
        items: [
          {
            id: 'run-command-group-legacy-one',
            type: 'run_command_group',
            callIds: ['legacy-one', 'legacy-two']
          }
        ]
      }
    ])
  })

  it('keeps separate staged occurrences when a later collaboration placement splits the supplied segments', () => {
    const args = { request: { action: 'append', transactionId: 'transaction' } }
    const current = run([
      call('first', 'apply_patch', args),
      call('command', 'run_command'),
      call('last', 'apply_patch', args)
    ])
    const whole = basicBlocks(current)[0]
    const left = basicBlocks(current, current.timeline.slice(0, 1))[0]
    const right = basicBlocks(current, current.timeline.slice(1))[0]
    expect(left.items[0].id).toBe(whole.items[0].id)
    expect(right.items[0].id).toBe(whole.items[1].id)
    expect(right.items[1].id).toBe('marker-last')
    expect(left.items[0].callIds).toEqual(['first'])
    expect(right.items[1].callIds).toEqual(['last'])
  })
})
