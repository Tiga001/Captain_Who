import { describe, expect, it } from 'vitest'
import { pdfUnavailableToolResult } from './managedPlaywrightToolResults'

describe('managed PDF failure projection', () => {
  it.each([
    ['cross_process_frame', 'cross-process embedded frame'],
    ['native_print_pending', 'close the page that started that print'],
    ['timed_out', 'close this page before retrying'],
    ['page_changed', 'result was discarded'],
    ['cancelled', 'no result was published'],
    ['output_too_large', 'smaller page'],
    ['native_print_failed', 'finish loading']
  ])('explains %s without exposing native diagnostics', (reason, recovery) => {
    const result = pdfUnavailableToolResult({
      content: [{ type: 'text', text: `Error: browser.pdf_unavailable:${reason} PRIVATE_CANARY` }],
      isError: true
    })
    expect(result).toMatchObject({
      isError: true,
      content: [{ type: 'text', text: expect.stringContaining(recovery) }],
      structuredContent: { code: 'browser.pdf_unavailable', status: 'unavailable', reason }
    })
    expect(JSON.stringify(result)).not.toContain('PRIVATE_CANARY')
  })

  it('does not pass unknown reasons or raw failures to the model', () => {
    const result = pdfUnavailableToolResult({
      content: [
        { type: 'text', text: 'browser.pdf_unavailable:unreviewed_reason /private/FILE_CANARY' }
      ],
      isError: true
    })
    expect(result).toEqual(pdfUnavailableToolResult())
    expect(JSON.stringify(result)).not.toContain('CANARY')
  })
})
