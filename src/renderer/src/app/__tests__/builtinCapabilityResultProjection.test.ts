import { BROWSER_DOWNLOAD_SCHEMA_VERSION, type AgentToolResult } from '@mycopilot/protocol'
import { describe, expect, it } from 'vitest'

import { projectBuiltinCapabilityToolResult } from '../../features/agentRun/builtinCapabilityResultProjection'

describe('builtin capability result projection', () => {
  it('drops an otherwise valid Download when the projection contains a private field', () => {
    const input: AgentToolResult = {
      callId: 'call-1',
      tool: 'browser_click',
      ok: true,
      result: {
        schemaVersion: 1,
        type: 'builtin_capability_tool',
        contentOmitted: true,
        status: 'completed',
        downloads: [
          {
            schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
            downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000',
            displayName: 'archive.zip',
            mimeType: 'application/zip',
            sizeBytes: 351,
            sha256: 'a'.repeat(64),
            createdAt: 1_000,
            source: 'agent'
          }
        ],
        privatePageResult: '/Users/private/Downloads/archive.zip'
      }
    }

    const projected = projectBuiltinCapabilityToolResult(input)
    expect(projected).toEqual({
      callId: 'call-1',
      tool: 'browser_click',
      ok: true,
      result: {
        schemaVersion: 1,
        type: 'builtin_capability_tool',
        contentOmitted: true,
        status: 'completed'
      }
    })
  })

  it('keeps a strict path-free Browser Download returned by Core', () => {
    const input: AgentToolResult = {
      callId: 'call-1',
      tool: 'browser_click',
      ok: true,
      result: {
        schemaVersion: 1,
        type: 'builtin_capability_tool',
        contentOmitted: true,
        status: 'completed',
        downloads: [
          {
            schemaVersion: BROWSER_DOWNLOAD_SCHEMA_VERSION,
            downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000',
            displayName: 'archive.zip',
            mimeType: 'application/zip',
            sizeBytes: 351,
            sha256: 'a'.repeat(64),
            createdAt: 1_000,
            source: 'agent'
          }
        ]
      }
    }

    expect(projectBuiltinCapabilityToolResult(input).result).toMatchObject({
      downloads: [
        {
          downloadId: 'browser-download:123e4567-e89b-42d3-a456-426614174000',
          displayName: 'archive.zip'
        }
      ]
    })
  })
})
