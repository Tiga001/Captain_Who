import { useState } from 'react'
import { createFileTreeIconResolver } from '@pierre/trees'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { getFrontendTheme } from '../../../config/frontendTheme'
import {
  ComposerAttachments,
  type ComposerAttachmentPresentation
} from '../components/ComposerAttachments'
import '../../../styles/global.css'
import '../components/ChatComposer.css'

const imageUrl =
  'data:image/svg+xml,' +
  encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="#69b1ff"/></svg>'
  )
const longName = 'A very long attachment filename that should preserve its extension.json'
const file = (name: string): ComposerAttachmentPresentation => ({
  id: name,
  name,
  kind: 'file',
  sizeBytes: 2048
})
const attachments: ComposerAttachmentPresentation[] = [
  file('README.md'),
  file('table.csv'),
  file('component.tsx'),
  file('report.pdf'),
  file(longName),
  { id: 'image', name: 'photo.svg', kind: 'image', sizeBytes: 256, previewUrl: imageUrl },
  file('notes.txt'),
  file('script.py')
]

function Harness({
  theme = 'classic-light',
  onPreview = vi.fn()
}: {
  theme?: 'classic-light' | 'classic-dark'
  onPreview?: (image: { alt: string; fileName: string; src: string }) => void
}) {
  const [items, setItems] = useState(attachments)
  return (
    <div
      style={{
        ...getFrontendCssVariables(undefined, getFrontendTheme(theme).tokens),
        width: 340,
        padding: 16,
        colorScheme: theme === 'classic-dark' ? 'dark' : 'light'
      }}
    >
      <ComposerAttachments
        attachments={items}
        label="Attachments"
        removeLabel="Remove"
        onRemove={(id) => setItems((previous) => previous.filter((item) => item.id !== id))}
        onPreview={onPreview}
      />
    </div>
  )
}

describe('Composer attachments', () => {
  it.each(['classic-light', 'classic-dark'] as const)(
    'stacks four compact files per column and keeps images between file groups in %s',
    async (theme) => {
      const screen = await render(<Harness theme={theme} />)
      const columns = [
        ...screen.container.querySelectorAll<HTMLElement>('.composer-attachment-column')
      ]
      expect(columns.map((column) => column.children.length)).toEqual([4, 1, 1, 2])
      const cards = [...screen.container.querySelectorAll<HTMLElement>('.composer-attachment')]
      expect(cards.map((card) => card.title.split(' · ')[0])).toEqual(
        attachments.map((item) => item.name)
      )
      const bounds = cards.map((card) => card.getBoundingClientRect())
      expect(bounds[0].height).toBe(28)
      expect(bounds[0].width).toBe(212)
      expect(bounds[3].left).toBe(bounds[0].left)
      expect(bounds[3].bottom - bounds[0].top).toBe(124)
      expect(bounds[4].left).toBeGreaterThan(bounds[3].right)
      expect(bounds[4].top).toBe(bounds[0].top)
      expect(bounds[5].height).toBe(126)
      expect(bounds[5].width).toBe(118)
      const strip = screen.container.querySelector<HTMLElement>('.chat-composer__attachments')!
      expect(strip.scrollWidth).toBeGreaterThan(strip.clientWidth)
      expect(getComputedStyle(strip).overflowX).toBe('auto')
      expect(strip.clientHeight).toBeLessThanOrEqual(128)

      const name = cards[4].querySelector<HTMLElement>('.composer-attachment__name')!
      const basename = name.querySelector<HTMLElement>('.composer-attachment__basename')!
      expect(name.textContent).toBe(longName)
      expect(getComputedStyle(name).whiteSpace).toBe('nowrap')
      expect(basename.scrollWidth).toBeGreaterThan(basename.clientWidth)
      expect(name.querySelector('.composer-attachment__extension')?.textContent).toBe('.json')
      expect(cards[4].title).toBe(`${longName} · 2 KiB`)
      expect(cards[4].querySelector('.composer-attachment__type')).toBeNull()

      const resolver = createFileTreeIconResolver({ colored: true, set: 'complete' })
      for (const card of cards.filter((item) => item.dataset.kind === 'file')) {
        const icon = card.querySelector<SVGElement>('.workspace-file-type-icon')!
        expect(icon.dataset.fileIcon).toBe(
          resolver.resolveIcon('file-tree-icon-file', card.title.split(' · ')[0]).token
        )
        expect(icon.querySelector('path')).not.toBeNull()
      }
      const markdownColor = getComputedStyle(
        cards[0].querySelector('.workspace-file-type-icon')!
      ).color
      expect(markdownColor).toBe(
        theme === 'classic-dark' ? 'rgb(94, 204, 113)' : 'rgb(25, 159, 67)'
      )
    }
  )

  it('keeps image previews and removal working as file columns reflow', async () => {
    const onPreview = vi.fn()
    const screen = await render(<Harness onPreview={onPreview} />)
    await screen.getByRole('button', { name: 'photo.svg', exact: true }).click()
    expect(onPreview).toHaveBeenCalledWith({
      alt: 'photo.svg',
      fileName: 'photo.svg',
      src: imageUrl
    })
    await screen.getByRole('button', { name: 'Remove README.md', exact: true }).click()
    expect(
      [...screen.container.querySelectorAll('.composer-attachment-column')].map(
        (column) => column.children.length
      )
    ).toEqual([4, 1, 2])
    expect(screen.container.querySelector('.composer-attachment__name')?.textContent).toBe(
      'table.csv'
    )
    await screen.getByRole('button', { name: 'Remove photo.svg', exact: true }).click()
    expect(
      [...screen.container.querySelectorAll('.composer-attachment-column')].map(
        (column) => column.children.length
      )
    ).toEqual([4, 2])
    expect(screen.container.querySelector('img')).toBeNull()
    await expect.element(screen.getByRole('button', { name: 'Remove script.py' })).toBeVisible()
  })
})
