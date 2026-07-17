import type { PDFDocumentLoadingTask } from 'pdfjs-dist'
import pdfWorkerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url'

type PdfJsRuntime = typeof import('pdfjs-dist')

let runtimePromise: Promise<PdfJsRuntime> | null = null

async function loadPdfJsRuntime(): Promise<PdfJsRuntime> {
  if (!runtimePromise) {
    runtimePromise = import('pdfjs-dist').catch((error) => {
      runtimePromise = null
      throw error
    })
  }
  return runtimePromise
}

export async function createWorkspacePdfLoadingTask(
  data: Uint8Array
): Promise<PDFDocumentLoadingTask> {
  const pdfjs = await loadPdfJsRuntime()
  pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerUrl
  return pdfjs.getDocument({
    data,
    enableXfa: false,
    isEvalSupported: false
  })
}
