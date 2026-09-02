import { describe, expect, it } from 'vitest'
import { formatModelConfigLabel } from '../../features/modelSelection/modelConfigPresentation'

describe('model config presentation', () => {
  it('uses the required display name as the complete user-facing identity', () => {
    expect(formatModelConfigLabel({ displayName: 'DeepSeek' })).toBe('DeepSeek')
  })

  it('trims presentation whitespace without exposing any other identity', () => {
    expect(formatModelConfigLabel({ displayName: '  Kimi K3 High  ' })).toBe('Kimi K3 High')
  })
})
