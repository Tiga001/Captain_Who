import { describe, expect, it } from 'vitest'
import {
  isAllowedUpdateRequest,
  parseUpdateSource,
  stripUpdateIdentityHeaders
} from '../updates/updateSource'

const source = new URL('https://updates.example.test/mac/arm64/')
describe('signed update configuration and request privacy', () => {
  it('reads only the generated generic stable configuration', () => {
    expect(
      parseUpdateSource(
        'provider: generic\nurl: https://updates.example.test/mac/arm64\nchannel: latest\nupdaterCacheDirName: captain-who-updater\nuseMultipleRangeRequest: false\n'
      ).href
    ).toBe(source.href)
  })
  it.each([
    'provider: github\nurl: https://example.test/',
    'provider: generic\nurl: http://example.test/',
    'provider: generic\nurl: https://user:password@example.test/',
    'provider: generic\nurl: https://example.test/?secret=key',
    'provider: generic\nurl: https://example.test/#fragment',
    'provider: generic\nurl: https://example.test/\nrequestHeaders: {Authorization: secret}',
    'provider: generic\nurl: https://example.test/\nchannel: beta',
    '[]',
    'null',
    'url: [bad]',
    'x'.repeat(16_385)
  ])('rejects non-public, non-stable or malformed configuration %#', (input) => {
    expect(() => parseUpdateSource(input)).toThrow()
  })
  it('allows only same-origin directory requests, including updater cache busters', () => {
    expect(isAllowedUpdateRequest(source + 'latest-mac.yml?noCache=123', source)).toBe(true)
    for (const url of [
      'http://updates.example.test/mac/arm64/file.zip',
      'https://evil.test/mac/arm64/file.zip',
      source + '../file.zip',
      source + '../../file.zip',
      'https://updates.example.test/mac/arm64-other/file.zip',
      'https://user@updates.example.test/mac/arm64/file.zip',
      source + 'file.zip#fragment',
      'bad-url'
    ]) {
      expect(isAllowedUpdateRequest(url, source)).toBe(false)
    }
  })
  it('strips account credentials and the SDK staging identifier without touching the input', () => {
    const headers = {
      Authorization: 'secret',
      Cookie: 'session',
      Referer: 'private',
      'Proxy-Authorization': 'secret',
      'X-User-Staging-ID': 'uuid',
      Accept: '*/*',
      Range: 'bytes=0-'
    }
    expect(stripUpdateIdentityHeaders(headers)).toEqual({ Accept: '*/*', Range: 'bytes=0-' })
    expect(headers.Authorization).toBe('secret')
  })
})
