import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { ImagePreviewProvider, useImagePreview } from '../../features/chat/components/ImagePreview'

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({ t: (key: string) => key })
}))

vi.mock('../../components/toast/ToastContext', () => ({
  useToast: () => ({ showToast: vi.fn() })
}))

function PreviewHarness({ release }: { release: () => void }) {
  const openImagePreview = useImagePreview()
  return (
    <button
      onClick={() =>
        openImagePreview({
          alt: 'generated image',
          fileName: 'generated-image.png',
          release,
          src: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='
        })
      }
      type="button"
    >
      open
    </button>
  )
}

describe('shared image preview source leases', () => {
  it('releases a retained temporary source when the existing viewer closes', async () => {
    const release = vi.fn()
    const screen = await render(
      <ImagePreviewProvider>
        <PreviewHarness release={release} />
      </ImagePreviewProvider>
    )

    await screen.getByRole('button', { name: 'open' }).click()
    await expect.element(screen.getByRole('dialog', { name: 'generated image' })).toBeVisible()
    expect(release).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'imagePreview.close' }).click()
    await expect
      .element(screen.getByRole('dialog', { name: 'generated image' }))
      .not.toBeInTheDocument()
    expect(release).toHaveBeenCalledOnce()
  })
})
