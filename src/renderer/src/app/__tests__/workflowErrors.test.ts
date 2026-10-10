import { describe, expect, it } from 'vitest'
import {
  workflowErrorDetail,
  workflowLoadErrorMessage,
  workflowOperationError,
  workflowUnavailableMemberModels
} from '../../features/workflows/workflowErrors'

describe('organization error diagnostics', () => {
  it.each([
    ['organization_template_invalid_format', 'format'],
    ['organization_template_too_large', 'too_large'],
    ['organization_template_read_failed', 'file']
  ])('classifies the public import error %s', (code, kind) => {
    expect(workflowOperationError({ message: code, data: { code } }, 'import')).toEqual({
      operation: 'import',
      kind
    })
    expect(workflowOperationError(new Error(code), 'import')).toEqual({ operation: 'import', kind })
  })

  it('distinguishes export writes from unrelated transfer failures without guessing from private text', () => {
    expect(
      workflowOperationError(new Error('organization_template_write_failed'), 'export')
    ).toEqual({ operation: 'export', kind: 'file' })
    for (const message of [
      'organization_template_dialog_failed',
      '/private/organization_template_invalid_format.md',
      'storage_unavailable'
    ]) {
      expect(workflowOperationError(new Error(message), 'import')).toEqual({
        operation: 'import',
        kind: 'unavailable'
      })
    }
  })

  it('explains unsupported board requests without leaking protocol or private error details', () => {
    const error = {
      code: -32602,
      message: 'Invalid params: unknown variant `getInstance`, expected one of `list`, `save`',
      data: { secret: 'private payload' },
      stack: '/private/customer/project'
    }
    expect(workflowLoadErrorMessage(error, 'zh-CN')).toBe(
      '后台服务无法识别组织读取请求，请重启应用后重试。'
    )
    expect(workflowLoadErrorMessage(error, 'en-US')).toContain('does not recognize')
    expect(
      workflowLoadErrorMessage({ ...error, message: 'private database failure' }, 'zh-CN')
    ).toBe('组织加载失败，请重试。')
  })

  it('distinguishes busy, timed-out and disconnected reads from an unknown failure', () => {
    expect(workflowLoadErrorMessage({ code: -32002 }, 'zh-CN')).toContain('后台服务繁忙')
    expect(
      workflowLoadErrorMessage({ code: -32001, data: { code: 'overloaded' } }, 'zh-CN')
    ).toContain('后台服务繁忙')
    expect(workflowLoadErrorMessage({ code: -32001 }, 'zh-CN')).toBe('组织加载失败，请重试。')
    expect(workflowLoadErrorMessage(new Error('Request timed out'), 'zh-CN')).toContain(
      '读取组织超时'
    )
    expect(
      workflowLoadErrorMessage(new Error('core-server exited with code 1 and signal null'), 'zh-CN')
    ).toContain('后台服务已断开')
    for (const error of [null, undefined, { data: { message: 'private payload' } }])
      expect(workflowLoadErrorMessage(error, 'zh-CN')).toBe('组织加载失败，请重试。')
  })

  const modelError = (members: unknown[]) => ({
    message: 'organization_member_models_unavailable',
    code: -32602,
    data: { code: 'organization_member_models_unavailable', members }
  })
  const member = {
    nodeId: 'member-a',
    nodeName: '方法抽取员',
    modelConfigId: 'model-a',
    modelDisplayName: 'DSflash'
  }

  it('groups unavailable models by configuration ID and preserves member order without duplicates', () => {
    expect(
      workflowUnavailableMemberModels(
        modelError([
          member,
          { ...member, nodeId: 'member-b', nodeName: '机制收集员' },
          member,
          { ...member, nodeId: 'member-c', modelConfigId: 'model-b', modelDisplayName: null },
          { ...member, nodeId: 'member-d', modelConfigId: 'model-c' }
        ])
      )
    ).toEqual([
      {
        modelConfigId: 'model-a',
        modelDisplayName: 'DSflash',
        memberNames: ['方法抽取员', '机制收集员']
      },
      { modelConfigId: 'model-b', modelDisplayName: null, memberNames: ['方法抽取员'] },
      { modelConfigId: 'model-c', modelDisplayName: 'DSflash', memberNames: ['方法抽取员'] }
    ])
  })

  it('rejects unrelated errors and malformed member payloads instead of guessing a model failure', () => {
    const valid = modelError([member])
    for (const error of [
      null,
      new Error('organization_member_models_unavailable'),
      { ...valid, code: -32000 },
      { ...valid, message: 'Storage unavailable' },
      { ...valid, data: { ...valid.data, code: 'different_error' } },
      modelError([]),
      modelError([null]),
      modelError([{ ...member, nodeId: '' }]),
      modelError([{ ...member, nodeName: 42 }]),
      modelError([{ ...member, modelConfigId: null }]),
      modelError([{ ...member, modelDisplayName: {} }]),
      modelError([member, { ...member, nodeName: ' ' }])
    ])
      expect(workflowUnavailableMemberModels(error)).toBeNull()
  })

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
