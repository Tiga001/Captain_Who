import { describe, expect, it } from 'vitest'
import { parseWorkspaceMarkdownDocument } from '../workspaceMarkdownDocument'

describe('parseWorkspaceMarkdownDocument', () => {
  it('separates YAML front matter from Markdown and preserves ordered metadata', () => {
    const result = parseWorkspaceMarkdownDocument(
      '\uFEFF---\r\nstatus: current\r\naudience:\r\n  - developers\r\n  - maintainers\r\nowner: engineering\r\nmetadata:\r\n  version: 2.2.0\r\n---\r\n# Context management\r\n\r\nBody\r\n'
    )

    expect(result).toEqual({
      body: '# Context management\r\n\r\nBody\r\n',
      metadata: [
        { key: 'status', value: { kind: 'text', text: 'current' } },
        {
          key: 'audience',
          value: { kind: 'list', items: ['developers', 'maintainers'] }
        },
        { key: 'owner', value: { kind: 'text', text: 'engineering' } },
        { key: 'metadata.version', value: { kind: 'text', text: '2.2.0' } }
      ],
      metadataTruncated: false
    })
  })

  it('keeps ordinary thematic breaks and unclosed front matter in the Markdown body', () => {
    expect(parseWorkspaceMarkdownDocument('---\nBody without a closing delimiter')).toEqual({
      body: '---\nBody without a closing delimiter',
      metadata: [],
      metadataTruncated: false
    })
    expect(parseWorkspaceMarkdownDocument('# Title\n\n---\n')).toEqual({
      body: '# Title\n\n---\n',
      metadata: [],
      metadataTruncated: false
    })
  })

  it('reports malformed or non-mapping metadata while keeping the Markdown body renderable', () => {
    expect(parseWorkspaceMarkdownDocument('---\nowner: [\n---\n# Title')).toMatchObject({
      body: '# Title',
      metadata: [],
      metadataError: 'invalid'
    })
    expect(parseWorkspaceMarkdownDocument('---\n- first\n- second\n---\n# Title')).toMatchObject({
      body: '# Title',
      metadata: [],
      metadataError: 'invalid'
    })
  })

  it('bounds oversized and highly populated metadata', () => {
    const oversized = parseWorkspaceMarkdownDocument(
      `---\ndescription: ${'x'.repeat(65 * 1024)}\n---\n# Title`
    )
    expect(oversized).toMatchObject({ body: '# Title', metadataError: 'too-large' })

    const manyFields = Array.from({ length: 105 }, (_, index) => `field_${index}: ${index}`).join(
      '\n'
    )
    const truncated = parseWorkspaceMarkdownDocument(`---\n${manyFields}\n---\n# Title`)
    expect(truncated.metadata).toHaveLength(100)
    expect(truncated.metadataTruncated).toBe(true)
  })
})
