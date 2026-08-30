import { useCallback, useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import { FileChangeDiffCard } from '../components/toolActivities/FileChangeDiffCard'
import '../ChatConversationPage.agent.css'
import '../../../styles/global.css'

const clipboardMocks = vi.hoisted(() => ({
  copyTextToClipboard: vi.fn(async () => undefined)
}))

vi.mock('../components/clipboard', () => clipboardMocks)

const translations: Record<string, string> = {
  'agent.fileChange.loadingPreview': '正在读取修改',
  'agent.fileChange.loadMorePreview': '加载更多',
  'chat.copy': '复制',
  'chat.copied': '已复制',
  'gitReview.diff.invalid': '这个文件的差异格式无效，无法安全显示。',
  'gitReview.diff.noHunks': '这个文件没有可显示的文本差异。',
  'gitReview.diff.tooLarge': '这个文件的差异过大，无法安全显示。'
}

vi.mock('../../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: string) => translations[key] ?? key
  })
}))

const REPLACEMENT_PATCH = [
  '--- a/src/example.ts',
  '+++ b/src/example.ts',
  '@@ -1,2 +1,2 @@',
  '-const answer: number = 41',
  '+const answer: number = 42',
  ' console.log(answer)'
].join('\n')

const ADDITION_PATCH = [
  '--- a/src/example.ts',
  '+++ b/src/example.ts',
  '@@ -1,1 +1,2 @@',
  ' export const ready = true',
  '+export const answer = 42'
].join('\n')

const DELETION_PATCH = [
  '--- a/src/example.ts',
  '+++ b/src/example.ts',
  '@@ -1,2 +1,1 @@',
  ' export const ready = true',
  '-export const obsolete = true'
].join('\n')

function CardHarness() {
  const [secondPatch, setSecondPatch] = useState(ADDITION_PATCH)
  const noOp = useCallback(() => undefined, [])
  return (
    <>
      <FileChangeDiffCard
        additions={1}
        complete
        deletions={1}
        error=""
        filePath="src/example.ts"
        hasMore={false}
        loading={false}
        onLoadMore={noOp}
        patch={REPLACEMENT_PATCH}
        toolCallId="first-patch"
      />
      <FileChangeDiffCard
        additions={1}
        complete
        deletions={0}
        error=""
        filePath="src/second.ts"
        hasMore={false}
        loading={false}
        onLoadMore={noOp}
        patch={secondPatch}
        toolCallId="second-patch"
      />
      <button type="button" onClick={() => setSecondPatch(`${ADDITION_PATCH}\n`)}>
        更新第二张卡片
      </button>
    </>
  )
}

describe('FileChangeDiffCard', () => {
  it('renders a themed split Diff with a fixed header and pane-local horizontal scrolling', async () => {
    const screen = await render(
      <FileChangeDiffCard
        additions={1}
        complete
        deletions={1}
        error=""
        filePath="src/example.ts"
        hasMore={false}
        loading={false}
        onLoadMore={() => undefined}
        patch={REPLACEMENT_PATCH}
        toolCallId="replacement-patch"
      />
    )

    await expect.element(screen.getByText('const answer: number = 41')).toBeVisible()
    await expect.element(screen.getByText('const answer: number = 42')).toBeVisible()
    const panes = Array.from(
      screen.container.querySelectorAll<HTMLElement>('.git-review__split-pane')
    )
    expect(panes).toHaveLength(2)
    panes.forEach((pane) => expect(window.getComputedStyle(pane).overflowX).toBe('auto'))

    const header = screen.container.querySelector<HTMLElement>('.file-change-diff-card__header')
    const body = screen.container.querySelector<HTMLElement>('.file-change-diff-card__body')
    expect(header?.contains(body)).toBe(false)
    expect(window.getComputedStyle(body!).maxHeight).toBe('320px')
    expect(window.getComputedStyle(body!).overflowY).toBe('auto')
    expect(screen.container.querySelector('.git-review__diff-line--addition')).not.toBeNull()
    expect(screen.container.querySelector('.git-review__diff-line--deletion')).not.toBeNull()

    await screen.getByRole('button', { name: '复制' }).click()
    expect(clipboardMocks.copyTextToClipboard).toHaveBeenCalledWith(REPLACEMENT_PATCH)
    await expect.element(screen.getByRole('button', { name: '已复制' })).toBeVisible()
  })

  it.each([
    ['addition', ADDITION_PATCH, 1, 0, 'new'],
    ['deletion', DELETION_PATCH, 0, 1, 'old']
  ] as const)(
    'uses one full-width pane for an %s-only patch',
    async (_, patch, additions, deletions, side) => {
      const screen = await render(
        <FileChangeDiffCard
          additions={additions}
          complete
          deletions={deletions}
          error=""
          filePath="src/example.ts"
          hasMore={false}
          loading={false}
          onLoadMore={() => undefined}
          patch={patch}
          toolCallId={`${side}-only-patch`}
        />
      )

      expect(
        screen.container.querySelector('.git-review__single-diff')?.getAttribute('data-side')
      ).toBe(side)
      expect(screen.container.querySelectorAll('.git-review__split-pane')).toHaveLength(0)
    }
  )

  it('renders only complete lines from a paginated patch prefix', async () => {
    const onLoadMore = vi.fn()
    const partialPatch = [
      '--- a/src/example.ts',
      '+++ b/src/example.ts',
      '@@ -1,2 +1,2 @@',
      '-const answer = 41',
      '+const answer = 42',
      ''
    ].join('\n')
    const screen = await render(
      <FileChangeDiffCard
        additions={1}
        complete={false}
        deletions={1}
        error=""
        filePath="src/example.ts"
        hasMore
        loading={false}
        onLoadMore={onLoadMore}
        patch={partialPatch}
        toolCallId="partial-patch"
      />
    )

    await expect.element(screen.getByText('const answer = 41')).toBeVisible()
    await expect.element(screen.getByText('const answer = 42')).toBeVisible()
    await screen.getByText('加载更多', { exact: true }).click()
    expect(onLoadMore).toHaveBeenCalledOnce()
  })

  it('keeps an unchanged terminal card mounted while a sibling Diff updates', async () => {
    const screen = await render(<CardHarness />)
    const firstCard = screen.container.querySelector<HTMLElement>(
      '.file-change-diff-card:first-of-type'
    )
    const firstDiff = firstCard?.querySelector<HTMLElement>('.git-review__split-diff')

    await screen.getByText('更新第二张卡片', { exact: true }).click()

    expect(
      screen.container.querySelector<HTMLElement>('.file-change-diff-card:first-of-type')
    ).toBe(firstCard)
    expect(firstCard?.querySelector<HTMLElement>('.git-review__split-diff')).toBe(firstDiff)
  })
})
