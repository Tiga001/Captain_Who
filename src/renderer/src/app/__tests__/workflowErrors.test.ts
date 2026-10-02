import { describe, expect, it } from 'vitest'
import {
  workflowErrorDetail,
  workflowOperationError
} from '../../features/workflows/workflowErrors'

describe('organization error diagnostics', () => {
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

  it('classifies a rejected configuration without exposing host details', () => {
    const error = Object.assign(new Error('Invalid organization fields'), {
      code: -32602,
      data: { definition: 'private draft content' }
    })
    expect(workflowOperationError(error, 'save')).toEqual({
      operation: 'save',
      kind: 'configuration'
    })
  })

  it('identifies duplicate member names for an actionable localized warning', () => {
    expect(
      workflowOperationError(
        new Error('Organization edit: organization_duplicate_member_name: internal details'),
        'save'
      )
    ).toEqual({ operation: 'save', kind: 'duplicate_member_name' })
  })

  it('classifies department naming problems without exposing internal diagnostics', () => {
    for (const kind of ['duplicate_department_name', 'department_name_separator'] as const)
      expect(
        workflowOperationError(new Error(`organization_${kind}: private details`), 'save')
      ).toEqual({ operation: 'save', kind })
  })

  it('uses a generic safe category for unexpected failures and non-error rejections', () => {
    for (const error of [
      new Error('Storage failed at /private/customer/data.sqlite: private token'),
      { code: -32000, data: { message: 'private payload' } },
      null,
      undefined
    ]) {
      expect(workflowOperationError(error, 'delete')).toEqual({
        operation: 'delete',
        kind: 'unavailable'
      })
    }
  })
})
