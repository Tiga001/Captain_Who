import { EventEmitter } from 'node:events'
import type { WebContents } from 'electron'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { printManagedGuestToPdf } from '../browser/ElectronGuestPdfPrinter'
import { ElectronGuestPdfStream } from '../browser/ElectronGuestPdfStream'

const PDF = Buffer.from('%PDF-1.7\nfixture\n%%EOF\n')
const PDF_PARAMS = {
  displayHeaderFooter: false,
  footerTemplate: '',
  generateDocumentOutline: false,
  generateTaggedPDF: false,
  headerTemplate: '',
  landscape: false,
  marginBottom: 0,
  marginLeft: 0,
  marginRight: 0,
  marginTop: 0,
  pageRanges: '',
  paperHeight: 11,
  paperWidth: 8.5,
  preferCSSPageSize: false,
  printBackground: false,
  scale: 1,
  transferMode: 'ReturnAsStream'
}
const guests = new Set<Guest>()

class Guest extends EventEmitter {
  destroyed = false
  readonly frame = {
    detached: false,
    frameToken: 'main',
    processId: 1,
    url: 'https://example.test/'
  }
  readonly mainFrame = { ...this.frame, framesInSubtree: [this.frame] }
  readonly printToPDF = vi.fn(async (): Promise<Buffer> => PDF)
  constructor() {
    super()
    guests.add(this)
  }
  isDestroyed(): boolean {
    return this.destroyed
  }
  destroy(): void {
    this.destroyed = true
    this.emit('destroyed')
  }
  asWebContents(): WebContents {
    return this as unknown as WebContents
  }
}

function deferred<T>(): { promise: Promise<T>; resolve(value: T): void } {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((accept) => {
    resolve = accept
  })
  return { promise, resolve }
}

afterEach(() => {
  for (const guest of guests) guest.destroy()
  guests.clear()
  vi.useRealTimers()
})

describe('managed guest native PDF printing', () => {
  it('rejects native cross-process frames before starting a print job', async () => {
    const guest = new Guest()
    guest.mainFrame.framesInSubtree.push({ ...guest.frame, frameToken: 'remote', processId: 2 })
    await expect(printManagedGuestToPdf(guest.asWebContents(), {})).rejects.toThrow(
      'cross_process_frame'
    )
    expect(guest.printToPDF).not.toHaveBeenCalled()
  })

  it('preserves native bytes and the exact print options', async () => {
    const guest = new Guest()
    const options = { printBackground: false, margins: { top: 0, bottom: 0, left: 0, right: 0 } }
    await expect(printManagedGuestToPdf(guest.asWebContents(), options)).resolves.toEqual(PDF)
    expect(guest.printToPDF).toHaveBeenCalledWith(options)
  })

  it.each(['navigation', 'frame replacement', 'new cross-process frame'])(
    'discards completed bytes after %s during printing',
    async (change) => {
      const guest = new Guest()
      const native = deferred<Buffer>()
      guest.printToPDF.mockReturnValueOnce(native.promise)
      const result = printManagedGuestToPdf(guest.asWebContents(), {})
      if (change === 'navigation') guest.emit('did-start-navigation')
      else if (change === 'frame replacement') guest.frame.frameToken = 'replacement'
      else
        guest.mainFrame.framesInSubtree.push({ ...guest.frame, frameToken: 'remote', processId: 2 })
      native.resolve(PDF)
      await expect(result).rejects.toThrow(/page_changed|cross_process_frame/u)
    }
  )

  it('retains the native reservation after cancellation and discards its late output', async () => {
    const guest = new Guest()
    const other = new Guest()
    const native = deferred<Buffer>()
    const cancellation = new AbortController()
    guest.printToPDF.mockReturnValueOnce(native.promise)
    const result = printManagedGuestToPdf(
      guest.asWebContents(),
      {},
      { signal: cancellation.signal }
    )
    cancellation.abort()
    await expect(result).rejects.toThrow('cancelled')
    await expect(printManagedGuestToPdf(other.asWebContents(), {})).rejects.toThrow(
      'native_print_pending'
    )
    expect(other.printToPDF).not.toHaveBeenCalled()
    native.resolve(PDF)
    await Promise.resolve()
    await expect(printManagedGuestToPdf(other.asWebContents(), {})).resolves.toEqual(PDF)
  })

  it('does not accumulate native jobs after timeout, and releases when the user closes the page', async () => {
    vi.useFakeTimers()
    const guest = new Guest()
    const other = new Guest()
    guest.printToPDF.mockReturnValueOnce(new Promise(() => undefined))
    const result = expect(
      printManagedGuestToPdf(guest.asWebContents(), {}, { timeoutMs: 50 })
    ).rejects.toThrow('timed_out')
    await vi.advanceTimersByTimeAsync(50)
    await result
    for (let index = 0; index < 3; index += 1) {
      await expect(printManagedGuestToPdf(other.asWebContents(), {})).rejects.toThrow(
        'native_print_pending'
      )
    }
    expect(guest.printToPDF).toHaveBeenCalledTimes(1)
    expect(other.printToPDF).not.toHaveBeenCalled()
    guest.destroy()
    await expect(printManagedGuestToPdf(other.asWebContents(), {})).resolves.toEqual(PDF)
  })

  it('releases native failures and rejects invalid PDF bytes', async () => {
    const guest = new Guest()
    guest.printToPDF.mockRejectedValueOnce(new Error('native failure'))
    await expect(printManagedGuestToPdf(guest.asWebContents(), {})).rejects.toThrow(
      'native_print_failed'
    )
    guest.printToPDF.mockResolvedValueOnce(Buffer.from('not a PDF'))
    await expect(printManagedGuestToPdf(guest.asWebContents(), {})).rejects.toThrow(
      'native_print_failed'
    )
    await expect(printManagedGuestToPdf(guest.asWebContents(), {})).resolves.toEqual(PDF)
  })

  it('reports an oversized PDF separately from a native print failure', async () => {
    const guest = new Guest()
    const bytes = Buffer.alloc(64 * 1024 * 1024 + 1)
    PDF.copy(bytes)
    guest.printToPDF.mockResolvedValueOnce(bytes)
    await expect(printManagedGuestToPdf(guest.asWebContents(), {})).rejects.toThrow(
      'output_too_large'
    )
    await expect(printManagedGuestToPdf(guest.asWebContents(), {})).resolves.toEqual(PDF)
  })
})

describe('private managed PDF CDP stream', () => {
  it('maps print defaults, bounds reads, isolates handles, and closes the stream', async () => {
    const guest = new Guest()
    guest.printToPDF.mockResolvedValueOnce(Buffer.concat([PDF, Buffer.alloc(2 * 1024 * 1024)]))
    const stream = new ElectronGuestPdfStream(guest.asWebContents())
    const other = new ElectronGuestPdfStream(new Guest().asWebContents())
    const printed = await stream.print(PDF_PARAMS)
    expect(guest.printToPDF).toHaveBeenCalledWith(
      expect.objectContaining({
        pageSize: { width: 8.5, height: 11 },
        margins: { top: 0, bottom: 0, left: 0, right: 0 }
      })
    )
    expect(() => other.read({ handle: printed.stream })).toThrow('native_print_failed')
    const first = stream.read({ handle: printed.stream, size: 3 * 1024 * 1024 })
    expect(Buffer.from(first.data as string, 'base64')).toHaveLength(1024 * 1024)
    expect(first.eof).toBe(false)
    stream.closeStream({ handle: printed.stream })
    expect(() => stream.read({ handle: printed.stream })).toThrow('native_print_failed')
    stream.dispose()
    other.dispose()
  })

  it('does not create a stream for a print completed after transport disposal', async () => {
    const guest = new Guest()
    const native = deferred<Buffer>()
    guest.printToPDF.mockReturnValueOnce(native.promise)
    const stream = new ElectronGuestPdfStream(guest.asWebContents())
    const result = stream.print(PDF_PARAMS)
    stream.dispose()
    native.resolve(PDF)
    await expect(result).rejects.toThrow('cancelled')
  })

  it('expires abandoned buffers and rejects unsupported print contracts without dispatch', async () => {
    vi.useFakeTimers()
    const guest = new Guest()
    const stream = new ElectronGuestPdfStream(guest.asWebContents())
    await expect(stream.print({ ...PDF_PARAMS, transferMode: 'ReturnAsBase64' })).rejects.toThrow(
      'native_print_failed'
    )
    expect(guest.printToPDF).not.toHaveBeenCalled()
    const printed = await stream.print(PDF_PARAMS)
    await vi.advanceTimersByTimeAsync(30_000)
    expect(() => stream.read({ handle: printed.stream })).toThrow('native_print_failed')
    stream.dispose()
  })
})
