import type { PrintToPDFOptions, WebContents } from 'electron'

const MAX_PDF_BYTES = 64 * 1024 * 1024
const PDF_PRINT_TIMEOUT_MS = 15_000

export type GuestPdfFailureReason =
  | 'cross_process_frame'
  | 'native_print_pending'
  | 'timed_out'
  | 'page_changed'
  | 'cancelled'
  | 'output_too_large'
  | 'native_print_failed'

export class GuestPdfError extends Error {
  constructor(readonly reason: GuestPdfFailureReason) {
    super(`browser.pdf_unavailable:${reason}`)
  }
}

// Electron has no native print cancellation API. Keep the reservation until the underlying
// operation actually settles or its WebContents is destroyed, including after client cancellation
// and transport replacement. Otherwise repeated timed-out requests accumulate native print jobs.
let pendingNativePrint: { guest: WebContents } | undefined

export async function printManagedGuestToPdf(
  guest: WebContents,
  options: PrintToPDFOptions,
  control: { signal?: AbortSignal; timeoutMs?: number } = {}
): Promise<Buffer> {
  if (control.signal?.aborted) throw new GuestPdfError('cancelled')
  const originalFrames = printableFrameIdentity(guest)
  if (pendingNativePrint) throw new GuestPdfError('native_print_pending')

  const reservation = { guest }
  pendingNativePrint = reservation
  const releaseNativeReservation = (): void => {
    if (pendingNativePrint === reservation) pendingNativePrint = undefined
    guest.removeListener('destroyed', releaseNativeReservation)
  }
  guest.once('destroyed', releaseNativeReservation)

  let pageChanged = false
  const navigationStarted = (): void => {
    pageChanged = true
  }
  guest.on('did-start-navigation', navigationStarted)
  let timer: ReturnType<typeof setTimeout> | undefined
  let cancelWait: (() => void) | undefined
  let targetDestroyed: (() => void) | undefined
  try {
    const nativePrint = guest.printToPDF(options)
    // Both branches consume settlement even when Promise.race has already returned to its caller.
    void nativePrint.then(releaseNativeReservation, releaseNativeReservation)
    const bytes = await Promise.race([
      nativePrint,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new GuestPdfError('timed_out')),
          control.timeoutMs ?? PDF_PRINT_TIMEOUT_MS
        )
        cancelWait = () => reject(new GuestPdfError('cancelled'))
        targetDestroyed = () => reject(new GuestPdfError('page_changed'))
        control.signal?.addEventListener('abort', cancelWait, { once: true })
        guest.once('destroyed', targetDestroyed)
        if (control.signal?.aborted) cancelWait()
      })
    ])
    if (control.signal?.aborted) throw new GuestPdfError('cancelled')
    if (pageChanged || printableFrameIdentity(guest) !== originalFrames) {
      throw new GuestPdfError('page_changed')
    }
    if (Buffer.isBuffer(bytes) && bytes.length > MAX_PDF_BYTES) {
      throw new GuestPdfError('output_too_large')
    }
    if (!Buffer.isBuffer(bytes) || !bytes.subarray(0, 5).equals(Buffer.from('%PDF-'))) {
      throw new GuestPdfError('native_print_failed')
    }
    return bytes
  } catch (error) {
    // A synchronous exception means that no native operation was created. Asynchronous failures
    // release themselves above. An aborted/expired wait must retain the native reservation.
    if (!(error instanceof GuestPdfError)) {
      releaseNativeReservation()
      throw new GuestPdfError('native_print_failed')
    }
    throw error
  } finally {
    if (timer) clearTimeout(timer)
    if (cancelWait) control.signal?.removeEventListener('abort', cancelWait)
    if (targetDestroyed) guest.removeListener('destroyed', targetDestroyed)
    guest.removeListener('did-start-navigation', navigationStarted)
  }
}

function printableFrameIdentity(guest: WebContents): string {
  try {
    if (guest.isDestroyed()) throw new GuestPdfError('page_changed')
    const mainFrame = guest.mainFrame
    const frames = mainFrame.framesInSubtree
    if (mainFrame.detached || frames.length === 0) throw new GuestPdfError('page_changed')
    // Chromium sends guest OOPIF print fragments to the outer WebContents compositor, while
    // Electron starts this PDF job on the guest compositor. The guest job never completes.
    // Inspect native frame identities; page script cannot forge these or hide a cross-site frame.
    if (frames.some((frame) => frame.processId !== mainFrame.processId)) {
      throw new GuestPdfError('cross_process_frame')
    }
    return frames
      .map((frame) => {
        if (frame.detached) throw new GuestPdfError('page_changed')
        return `${frame.processId}:${frame.frameToken}:${frame.url}`
      })
      .join('\n')
  } catch (error) {
    if (error instanceof GuestPdfError) throw error
    throw new GuestPdfError('page_changed')
  }
}
