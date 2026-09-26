import { describe, expect, it } from 'vitest'
import { workflowErrorDetail } from '../../features/workflows/workflowErrors'

describe('workflow error diagnostics', () => {
  it('keeps the host message and code without serializing payloads or stacks', () => {
    const error = Object.assign(new Error('Storage read failed'), {
      code: -32000,
      data: { graph: 'private data' }
    })
    expect(workflowErrorDetail(error)).toBe('[-32000] Storage read failed')
    expect(workflowErrorDetail({ data: { message: 'not a public message' } })).toBe('')
  })

  it('bounds diagnostic messages and removes control characters', () => {
    expect(workflowErrorDetail('a\0b\nc')).toBe('ab\nc')
    expect(workflowErrorDetail('x'.repeat(1300))).toBe(`${'x'.repeat(1200)}…`)
    expect(workflowErrorDetail({ message: 'Invalid response', code: Number.NaN })).toBe(
      'Invalid response'
    )
  })
})
