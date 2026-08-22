import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import '../../../styles/global.css'
import '../FilesPanel.css'

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

const { WorkspacePdfPreview } = await import('../WorkspacePdfPreview')

describe('WorkspacePdfPreview', () => {
  it('loads a real PDF.js worker and paints the current page into the visible canvas', async () => {
    const data = createSinglePagePdf()
    const screen = await render(
      <div style={{ height: 520, width: 420 }}>
        <WorkspacePdfPreview
          content={{
            data,
            mimeType: 'application/pdf',
            modifiedAtMs: 1,
            path: 'fixture.pdf',
            sizeBytes: data.byteLength
          }}
          initialPage={1}
          onPageChange={vi.fn()}
          onRetry={vi.fn()}
          path="fixture.pdf"
        />
      </div>
    )

    try {
      // A canvas has a non-zero default size before PDF.js paints it. Wait for the component's
      // real render terminal state so this remains a worker integration test instead of a DOM race.
      await expect
        .poll(() => screen.container.querySelector('.files-panel__pdf-loading'), {
          interval: 50,
          timeout: 15_000
        })
        .toBeNull()
      await expect
        .poll(() => screen.container.querySelector('.files-panel__pdf-pager')?.textContent, {
          interval: 50,
          timeout: 15_000
        })
        .toContain('1/1')

      const canvas = screen.container.querySelector<HTMLCanvasElement>('.files-panel__pdf-page')
      if (!canvas) throw new Error('PDF preview did not render a canvas')
      const context = canvas.getContext('2d')
      if (!context) throw new Error('PDF preview canvas has no 2D context')
      const pixel = context.getImageData(
        Math.floor(canvas.width / 2),
        Math.floor(canvas.height / 2),
        1,
        1
      ).data

      expect(Array.from(pixel)).toEqual([255, 0, 0, 255])
      await expect
        .element(screen.getByRole('button', { name: 'files.pdf.previousPage' }))
        .toBeDisabled()
      await expect
        .element(screen.getByRole('button', { name: 'files.pdf.nextPage' }))
        .toBeDisabled()
    } finally {
      screen.unmount()
    }
  })
})

function createSinglePagePdf(): Uint8Array {
  const contentStream = '1 0 0 rg\n20 20 160 260 re\nf\n'
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Resources << >> /Contents 4 0 R >>',
    `<< /Length ${contentStream.length} >>\nstream\n${contentStream}endstream`
  ]

  let source = '%PDF-1.4\n'
  const offsets = objects.map((object, index) => {
    const offset = source.length
    source += `${index + 1} 0 obj\n${object}\nendobj\n`
    return offset
  })
  const xrefOffset = source.length
  source += `xref\n0 ${objects.length + 1}\n`
  source += '0000000000 65535 f \n'
  for (const offset of offsets) {
    source += `${String(offset).padStart(10, '0')} 00000 n \n`
  }
  source += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\n`
  source += `startxref\n${xrefOffset}\n%%EOF\n`
  return new TextEncoder().encode(source)
}
