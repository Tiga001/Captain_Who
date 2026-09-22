import { describe, expect, it } from 'vitest'
import { buildMessageContentWithWorkspaceMentions } from '../workspaceMentions'

describe('workspace mention message content', () => {
  const mention = {
    id: 'folder:file.ts',
    projectId: 'project',
    folderId: 'folder',
    alias: 'app',
    displayName: 'file.ts',
    path: 'src/file.ts',
    displayPath: 'app/src/file.ts',
    kind: 'file' as const
  }

  it('keeps logical paths and visible labels without exposing native paths', () => {
    expect(buildMessageContentWithWorkspaceMentions('', [mention])).toBe(
      '[file.ts](@workspace/app/src/file.ts)'
    )
  })

  it('appends references after ordinary text', () => {
    expect(buildMessageContentWithWorkspaceMentions('Inspect this', [mention])).toBe(
      'Inspect this\n\n[file.ts](@workspace/app/src/file.ts)'
    )
  })
})
