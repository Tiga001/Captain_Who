import { describe, expect, it } from 'vitest'
import { normalizeBrowserUrl } from '../../browser/browserUrl'

describe('normalizeBrowserUrl', () => {
  it.each([
    ['localhost:3000', 'http://localhost:3000/'],
    [' localhost:5173/login ', 'http://localhost:5173/login'],
    ['localhost?debug=1', 'http://localhost/?debug=1'],
    ['localhost:3000#section', 'http://localhost:3000/#section'],
    ['127.0.0.1:8080/test', 'http://127.0.0.1:8080/test'],
    ['[::1]:3000/login', 'http://[::1]:3000/login'],
    ['example.com:8080', 'https://example.com:8080/'],
    ['example.com:8080/path?query=1#result', 'https://example.com:8080/path?query=1#result'],
    ['app.example.com:8443', 'https://app.example.com:8443/'],
    ['example.com', 'https://example.com/'],
    ['http://localhost:3000', 'http://localhost:3000/'],
    ['https://localhost:3000/login', 'https://localhost:3000/login'],
    ['https://example.com:8443/path', 'https://example.com:8443/path']
  ])('normalizes %s into %s', (input, expected) => {
    expect(normalizeBrowserUrl(input)).toBe(expected)
  })

  it.each([
    '',
    '   ',
    'not a valid address',
    'localhost:65536',
    'example.com:abc',
    'https://',
    'javascript:alert(1)',
    'data:text/html,test',
    'file:///private/report.pdf',
    'ftp://example.com/file',
    'mailto:user@example.com'
  ])('rejects invalid addresses and unsupported schemes: %s', (input) => {
    expect(normalizeBrowserUrl(input)).toBeNull()
  })
})
