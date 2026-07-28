import type { AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView } from '../chatTypes'
import { upsertWebSearchActivityFromResult } from '../agentWebSearch'

function run(): ChatAgentRunView {
  return {
    runId: 'run-web-contract',
    status: 'running',
    toolDefinitions: [],
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    timeline: []
  }
}

describe('web Tool result consumer contract', () => {
  it('retains favicon, relevance, chronology and presentation metadata consumed by Renderer', () => {
    const result: AgentToolResult = {
      callId: 'call-web-search',
      tool: 'web_search',
      ok: true,
      result: {
        query: 'projection contracts',
        provider: 'tavily',
        answer: 'A concise answer.',
        results: [
          {
            title: 'Lower relevance',
            url: 'https://lower.example/article',
            content: 'Lower snippet',
            score: 0.3,
            publishedDate: '2026-07-27',
            favicon: 'https://lower.example/favicon.ico'
          },
          {
            title: 'Higher relevance',
            url: 'https://higher.example/article',
            content: 'Higher snippet',
            score: 0.9,
            publishedDate: '2026-07-28',
            favicon: 'https://higher.example/favicon.ico'
          }
        ],
        responseTime: 0.42,
        truncated: true
      }
    }

    const [activity] = upsertWebSearchActivityFromResult(run(), result) ?? []

    expect(activity).toMatchObject({
      callId: 'call-web-search',
      query: 'projection contracts',
      provider: 'tavily',
      answer: 'A concise answer.',
      responseTime: 0.42,
      truncated: true
    })
    expect(activity.sources).toEqual([
      expect.objectContaining({
        title: 'Higher relevance',
        faviconUrl: 'https://higher.example/favicon.ico',
        score: 0.9,
        publishedDate: '2026-07-28'
      }),
      expect.objectContaining({
        title: 'Lower relevance',
        faviconUrl: 'https://lower.example/favicon.ico',
        score: 0.3,
        publishedDate: '2026-07-27'
      })
    ])
  })
})
