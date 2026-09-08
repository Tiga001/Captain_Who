import { randomUUID } from 'node:crypto'
import type { PrintToPDFOptions, WebContents } from 'electron'
import { GuestPdfError, printManagedGuestToPdf } from './ElectronGuestPdfPrinter'

const STREAM_PREFIX = 'mycopilot-guest-pdf-'
const STREAM_CHUNK_BYTES = 1024 * 1024
const STREAM_IDLE_TIMEOUT_MS = 30_000

/** Private CDP compatibility stream for Playwright's page.pdf() on this exact admitted guest. */
export class ElectronGuestPdfStream {
  private readonly cancellation = new AbortController()
  private stream?: { bytes: Buffer; handle: string; offset: number }
  private timer?: ReturnType<typeof setTimeout>

  constructor(private readonly guest: WebContents) {}

  async print(params: Record<string, unknown> | undefined): Promise<{ stream: string }> {
    if (this.stream) throw new GuestPdfError('native_print_pending')
    const bytes = await printManagedGuestToPdf(this.guest, printOptions(params), {
      signal: this.cancellation.signal
    })
    if (this.cancellation.signal.aborted) throw new GuestPdfError('cancelled')
    const handle = `${STREAM_PREFIX}${randomUUID()}`
    this.stream = { bytes, handle, offset: 0 }
    this.armExpiry()
    return { stream: handle }
  }

  ownsHandle(params: Record<string, unknown> | undefined): boolean {
    return typeof params?.handle === 'string' && params.handle.startsWith(STREAM_PREFIX)
  }

  read(params: Record<string, unknown> | undefined): Record<string, unknown> {
    const stream = this.requireStream(params)
    if (
      Object.keys(params!).some((key) => !['handle', 'offset', 'size'].includes(key)) ||
      (params!.offset !== undefined &&
        (!Number.isSafeInteger(params!.offset) || Number(params!.offset) < 0)) ||
      (params!.size !== undefined &&
        (!Number.isSafeInteger(params!.size) || Number(params!.size) <= 0))
    ) {
      throw new GuestPdfError('native_print_failed')
    }
    const offset = Math.min(Number(params!.offset ?? stream.offset), stream.bytes.length)
    const size = Math.min(Number(params!.size ?? STREAM_CHUNK_BYTES), STREAM_CHUNK_BYTES)
    const end = Math.min(offset + size, stream.bytes.length)
    const data = stream.bytes.subarray(offset, end).toString('base64')
    stream.offset = end
    this.armExpiry()
    return { base64Encoded: true, data, eof: end === stream.bytes.length }
  }

  closeStream(params: Record<string, unknown> | undefined): Record<string, never> {
    this.requireStream(params)
    if (Object.keys(params!).length !== 1) throw new GuestPdfError('native_print_failed')
    this.clearStream()
    return {}
  }

  dispose(): void {
    this.cancellation.abort()
    this.clearStream()
  }

  private requireStream(
    params: Record<string, unknown> | undefined
  ): NonNullable<typeof this.stream> {
    if (!this.stream || params?.handle !== this.stream.handle) {
      throw new GuestPdfError('native_print_failed')
    }
    return this.stream
  }

  private armExpiry(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = setTimeout(() => this.clearStream(), STREAM_IDLE_TIMEOUT_MS)
    this.timer.unref()
  }

  private clearStream(): void {
    if (this.timer) clearTimeout(this.timer)
    this.timer = undefined
    this.stream = undefined
  }
}

function printOptions(params: Record<string, unknown> | undefined): PrintToPDFOptions {
  if (!params || params.transferMode !== 'ReturnAsStream') {
    throw new GuestPdfError('native_print_failed')
  }
  const booleanFields = [
    'landscape',
    'displayHeaderFooter',
    'printBackground',
    'preferCSSPageSize',
    'generateTaggedPDF',
    'generateDocumentOutline'
  ] as const
  const stringFields = ['pageRanges', 'headerTemplate', 'footerTemplate'] as const
  const numericFields = [
    'scale',
    'paperWidth',
    'paperHeight',
    'marginTop',
    'marginBottom',
    'marginLeft',
    'marginRight'
  ] as const
  const allowed = new Set<string>([
    'transferMode',
    ...booleanFields,
    ...stringFields,
    ...numericFields
  ])
  if (
    Object.keys(params).some((key) => !allowed.has(key)) ||
    booleanFields.some((key) => typeof params[key] !== 'boolean') ||
    stringFields.some((key) => typeof params[key] !== 'string') ||
    numericFields.some((key) => typeof params[key] !== 'number' || !Number.isFinite(params[key]))
  ) {
    throw new GuestPdfError('native_print_failed')
  }
  const options: PrintToPDFOptions = {}
  for (const key of booleanFields) options[key] = params[key] as boolean
  for (const key of stringFields) options[key] = params[key] as string
  return {
    ...options,
    scale: params.scale as number,
    pageSize: { width: params.paperWidth as number, height: params.paperHeight as number },
    margins: {
      top: params.marginTop as number,
      bottom: params.marginBottom as number,
      left: params.marginLeft as number,
      right: params.marginRight as number
    }
  }
}
