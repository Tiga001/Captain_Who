import { describe, expect, it } from 'vitest'
import { resolveWorkspaceMarkdownLink } from '../workspaceMarkdownLinks'

describe('resolveWorkspaceMarkdownLink', () => {
  it('resolves relative, root-relative, encoded, and anchored workspace paths', () => {
    expect(
      resolveWorkspaceMarkdownLink('guides/current/intro.md', '../../docs/setup.md#install')
    ).toEqual({
      anchor: 'install',
      kind: 'workspace-file',
      path: 'docs/setup.md'
    })
    expect(resolveWorkspaceMarkdownLink('guides/intro.md', '/README.md')).toEqual({
      kind: 'workspace-file',
      path: 'README.md'
    })
    expect(resolveWorkspaceMarkdownLink('guides/intro.md', './API%20Guide.md')).toEqual({
      kind: 'workspace-file',
      path: 'guides/API Guide.md'
    })
    expect(resolveWorkspaceMarkdownLink('guides/intro.md', '#配置')).toEqual({
      anchor: '配置',
      kind: 'anchor'
    })
  })

  it('keeps HTTP links external and rejects unsafe or escaping paths', () => {
    expect(resolveWorkspaceMarkdownLink('README.md', 'https://example.com/docs')).toEqual({
      kind: 'external',
      url: 'https://example.com/docs'
    })
    expect(resolveWorkspaceMarkdownLink('README.md', '//example.com/docs')).toEqual({
      kind: 'external',
      url: 'https://example.com/docs'
    })
    expect(resolveWorkspaceMarkdownLink('README.md', '../outside.md')).toEqual({
      kind: 'unsupported'
    })
    expect(resolveWorkspaceMarkdownLink('docs/README.md', '%2e%2e/%2e%2e/outside.md')).toEqual({
      kind: 'unsupported'
    })
    expect(resolveWorkspaceMarkdownLink('README.md', 'javascript:alert(1)')).toEqual({
      kind: 'unsupported'
    })
    expect(resolveWorkspaceMarkdownLink('README.md', 'file:///tmp/file.md')).toEqual({
      kind: 'unsupported'
    })
    expect(resolveWorkspaceMarkdownLink('README.md', '%E0%A4%A')).toEqual({
      kind: 'unsupported'
    })
  })
})
