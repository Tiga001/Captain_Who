import type { HumanInteractionAnswer } from '@mycopilot/protocol'
import { useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { page, userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'
import { getFrontendCssVariables } from '../../../config/frontendConfig'
import { getFrontendTheme } from '../../../config/frontendTheme'
import { ApprovalDialogShell } from '../../chat/components/ApprovalDialogShell'
import { HumanInteractionPanel, type HumanInteractionPanelProps } from '../HumanInteractionPanel'
import '../../../styles/global.css'
import '../../chat/ChatConversationPage.approvals.css'

vi.mock('../../../config/FrontendConfigProvider', async () => {
  const { getTranslation } = await import('../../../config/languageRegistry')
  return {
    useFrontendConfig: () => ({
      language: 'zh-CN',
      t: (key: Parameters<typeof getTranslation>[1]) => getTranslation('zh-CN', key)
    })
  }
})

const request: HumanInteractionPanelProps['request'] = {
  requestId: 'request-panel',
  mode: 'async',
  questions: [
    {
      id: 'appearance',
      title: '你更喜欢哪一种外观？',
      options: [
        { id: 'light', label: '明亮外观' },
        { id: 'dark', label: '深色外观' }
      ]
    },
    { id: 'details', title: '还有哪些需求？', options: null },
    { id: 'later', title: '未来计划是什么？', options: null }
  ]
}
const completed: HumanInteractionPanelProps['answers'] = {
  appearance: { kind: 'option', questionId: 'appearance', optionId: 'light' },
  details: { kind: 'text', questionId: 'details', text: '支持中文' },
  later: { kind: 'skipped', questionId: 'later' }
}

function Fixture({
  initialAnswers = {},
  onAnswerChange = () => {},
  onPageChange = () => {},
  ...props
}: Partial<HumanInteractionPanelProps> & {
  initialAnswers?: HumanInteractionPanelProps['answers']
}) {
  const [answers, setAnswers] = useState(initialAnswers)
  const [pageIndex, setPageIndex] = useState(0)
  return (
    <HumanInteractionPanel
      request={request}
      pageIndex={pageIndex}
      answers={answers}
      canSubmit
      isSubmitting={false}
      onSubmit={() => {}}
      onIgnore={() => {}}
      onMinimize={() => {}}
      {...props}
      onAnswerChange={(answer: HumanInteractionAnswer) => {
        setAnswers((current) => ({ ...current, [answer.questionId]: answer }))
        onAnswerChange(answer)
      }}
      onPageChange={(index) => {
        setPageIndex(index)
        onPageChange(index)
      }}
    />
  )
}

let previousRootStyle: string | null
beforeEach(async () => {
  previousRootStyle = document.documentElement.getAttribute('style')
  const theme = getFrontendTheme('classic-light')
  for (const [key, value] of Object.entries(getFrontendCssVariables(undefined, theme.tokens))) {
    document.documentElement.style.setProperty(key, value)
  }
  await page.viewport(1000, 820)
})
afterEach(async () => {
  if (previousRootStyle === null) document.documentElement.removeAttribute('style')
  else document.documentElement.setAttribute('style', previousRootStyle)
  await page.viewport(1280, 720)
})

describe('HumanInteractionPanel', () => {
  it('preserves pages and lets a user replace option, text and skipped drafts before one final submit', async () => {
    const submit = vi.fn()
    const change = vi.fn()
    const screen = await render(<Fixture onSubmit={submit} onAnswerChange={change} />)
    const previous = screen.getByRole('button', { name: '上一题' })
    const next = screen.getByRole('navigation').getByRole('button', { name: '下一题' })
    const primary = page
      .elementLocator(screen.container.querySelector<HTMLElement>('footer')!)
      .getByRole('button', { name: /下一题|提交/ })
    const send = screen.getByRole('button', { name: '提交', exact: true })
    await expect.element(previous).toBeDisabled()
    await expect.element(primary).toBeDisabled()
    await screen.getByRole('button', { name: '明亮外观' }).click()
    await expect.element(primary).toHaveTextContent('下一题')
    await primary.click()
    await screen.getByRole('textbox').fill('支持中文')
    await primary.click()
    await expect.element(next).toBeDisabled()
    await screen.getByRole('button', { name: '跳过', exact: true }).click()
    await expect
      .element(screen.getByRole('button', { name: '已跳过', exact: true }))
      .toHaveAttribute('aria-pressed', 'true')
    await expect.element(send).toBeEnabled()
    expect(submit).not.toHaveBeenCalled()
    await previous.click()
    await expect.element(screen.getByRole('textbox')).toHaveValue('支持中文')
    await previous.click()
    await expect
      .element(screen.getByRole('button', { name: '明亮外观' }))
      .toHaveAttribute('aria-pressed', 'true')
    await screen.getByRole('textbox').fill('另一种外观')
    await expect
      .element(screen.getByRole('button', { name: '明亮外观' }))
      .toHaveAttribute('aria-pressed', 'false')
    await screen.getByRole('button', { name: '跳过', exact: true }).click()
    await expect.element(screen.getByRole('textbox')).toHaveValue('')
    await screen.getByRole('button', { name: '深色外观' }).click()
    await expect
      .element(screen.getByRole('button', { name: '跳过', exact: true }))
      .toHaveAttribute('aria-pressed', 'false')
    await screen.getByRole('textbox').fill('  ')
    await expect.element(primary).toBeDisabled()
    await screen.getByRole('button', { name: '深色外观' }).click()
    await send.click()
    expect(submit).toHaveBeenCalledTimes(1)
    expect(change).toHaveBeenLastCalledWith({
      kind: 'option',
      questionId: 'appearance',
      optionId: 'dark'
    })
    await expect
      .element(screen.getByRole('button', { name: '深色外观' }))
      .toHaveAttribute('aria-pressed', 'true')
  })

  it('keeps async minimize and ignore independent and omits them for a blocking request', async () => {
    const submit = vi.fn()
    const ignore = vi.fn()
    const minimize = vi.fn()
    const change = vi.fn()
    const screen = await render(
      <Fixture
        sourceRunEnded
        onSubmit={submit}
        onIgnore={ignore}
        onMinimize={minimize}
        onAnswerChange={change}
      />
    )
    await expect.element(screen.getByText('本轮已结束，提交回答后继续')).toBeVisible()
    await screen.getByRole('button', { name: '最小化交互' }).click()
    await screen.getByRole('button', { name: '忽略全部' }).click()
    expect(minimize).toHaveBeenCalledTimes(1)
    expect(ignore).toHaveBeenCalledTimes(1)
    expect(submit).not.toHaveBeenCalled()
    expect(change).not.toHaveBeenCalled()
    await screen.rerender(
      <Fixture
        request={{ ...request, mode: 'sync' }}
        sourceRunEnded
        onIgnore={ignore}
        onMinimize={minimize}
      />
    )
    expect(screen.getByRole('button', { name: '最小化交互' }).elements()).toHaveLength(0)
    expect(screen.getByRole('button', { name: '忽略全部' }).elements()).toHaveLength(0)
    expect(screen.getByText('本轮已结束，提交回答后继续').elements()).toHaveLength(0)
  })

  it('prevents duplicate actions while busy, then preserves the frozen payload for an explicit retry', async () => {
    const submit = vi.fn()
    const ignore = vi.fn()
    const screen = await render(
      <Fixture initialAnswers={completed} isSubmitting onSubmit={submit} onIgnore={ignore} />
    )
    await expect.element(screen.getByRole('dialog')).toHaveAttribute('aria-busy', 'true')
    for (const name of ['明亮外观', '跳过', '忽略全部', '提交']) {
      const button = screen.getByRole('button', { name, exact: true })
      await expect.element(button).toBeDisabled()
      ;(button.element() as HTMLButtonElement).click()
    }
    await expect.element(screen.getByRole('textbox')).toBeDisabled()
    expect(submit).not.toHaveBeenCalled()
    expect(ignore).not.toHaveBeenCalled()
    await screen.getByRole('button', { name: '下一题' }).click()
    await expect.element(screen.getByRole('textbox')).toHaveValue('支持中文')
    await screen.rerender(
      <Fixture
        initialAnswers={completed}
        isDraftLocked
        onSubmit={submit}
        onIgnore={undefined}
        error="提交结果尚未确认，请重试。"
      />
    )
    await expect.element(screen.getByRole('alert')).toHaveTextContent('提交结果尚未确认')
    await expect.element(screen.getByRole('textbox')).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: '忽略全部' })).toBeDisabled()
    await screen.getByRole('button', { name: '提交', exact: true }).click()
    expect(submit).toHaveBeenCalledTimes(1)
    await screen.rerender(
      <Fixture initialAnswers={completed} isDraftLocked canSubmit={false} onIgnore={ignore} />
    )
    await expect.element(screen.getByRole('button', { name: '提交', exact: true })).toBeDisabled()
    await screen.getByRole('button', { name: '忽略全部' }).click()
    expect(ignore).toHaveBeenCalledTimes(1)
  })

  it('keeps single-line input Enter, Escape and IME confirmation away from form and composer actions', async () => {
    const outerKeyDown = vi.fn()
    const outerKeyUp = vi.fn()
    const formSubmit = vi.fn()
    const submit = vi.fn()
    const ignore = vi.fn()
    const screen = await render(
      <form
        onKeyDown={outerKeyDown}
        onKeyUp={outerKeyUp}
        onSubmit={(event) => {
          event.preventDefault()
          formSubmit()
        }}
      >
        <Fixture initialAnswers={completed} onSubmit={submit} onIgnore={ignore} />
      </form>
    )
    const input = screen.getByRole('textbox')
    await input.fill('第一行')
    await userEvent.keyboard('{End}{Enter}第二行{Escape}')
    await expect.element(input).toHaveValue('第一行第二行')
    const element = input.element()
    element.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }))
    element.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Enter', isComposing: true, keyCode: 229, bubbles: true })
    )
    element.dispatchEvent(
      new KeyboardEvent('keyup', { key: 'Enter', isComposing: true, keyCode: 229, bubbles: true })
    )
    element.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }))
    const escape = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
    element.dispatchEvent(escape)
    expect(escape.defaultPrevented).toBe(true)
    expect(outerKeyDown).not.toHaveBeenCalled()
    expect(outerKeyUp).not.toHaveBeenCalled()
    expect(formSubmit).not.toHaveBeenCalled()
    expect(submit).not.toHaveBeenCalled()
    expect(ignore).not.toHaveBeenCalled()
    await expect
      .element(screen.getByRole('button', { name: '跳过', exact: true }))
      .toHaveAttribute('aria-pressed', 'false')
    expect(
      [...screen.container.querySelectorAll('button')].every((button) => button.type === 'button')
    ).toBe(true)
  })

  it('does not truncate large batches and requires valid answers to every original question', async () => {
    const changePage = vi.fn()
    const submit = vi.fn()
    const questions = Array.from({ length: 70 }, (_, index) => ({
      id: `question-${index}`,
      title: `完整问题 ${index + 1}`,
      options: null
    }))
    const answers = Object.fromEntries(
      questions.map((question) => [
        question.id,
        { kind: 'skipped' as const, questionId: question.id }
      ])
    )
    const screen = await render(
      <Fixture request={{ ...request, questions }} pageIndex={69} answers={answers} />
    )
    await expect.element(screen.getByText('70 / 70')).toBeVisible()
    await expect.element(screen.getByRole('heading', { name: '完整问题 70' })).toBeVisible()
    await expect.element(screen.getByRole('button', { name: '提交', exact: true })).toBeEnabled()
    await screen.rerender(
      <Fixture
        request={{ ...request, questions }}
        pageIndex={69}
        answers={{ ...answers, 'question-0': undefined }}
        onPageChange={changePage}
        onSubmit={submit}
      />
    )
    await expect
      .element(screen.getByRole('button', { name: '提交', exact: true }))
      .not.toBeInTheDocument()
    await expect
      .element(
        page
          .elementLocator(screen.container.querySelector<HTMLElement>('footer')!)
          .getByRole('button', { name: '下一题' })
      )
      .toBeEnabled()
    await page
      .elementLocator(screen.container.querySelector<HTMLElement>('footer')!)
      .getByRole('button', { name: '下一题' })
      .click()
    expect(changePage).toHaveBeenCalledWith(0)
    expect(submit).not.toHaveBeenCalled()
  })

  it('matches approval typography and surface tokens while keeping long content inside a narrow panel', async () => {
    const title = '完整的长问题标题'.repeat(24)
    const longRequest = {
      ...request,
      questions: [
        {
          id: 'long',
          title,
          options: Array.from({ length: 12 }, (_, index) => ({
            id: `option-${index}`,
            label: `选项 ${index + 1}：${'完整说明文字'.repeat(6)}`
          }))
        }
      ]
    }
    const screen = await render(
      <div style={{ width: 320 }}>
        <Fixture request={longRequest} />
        <ApprovalDialogShell
          approvalKind="standard"
          approveLabel="允许"
          isSubmitting={false}
          onApprove={() => {}}
          onReject={() => {}}
          onRejectMessageChange={() => {}}
          rejectLabel="拒绝"
          rejectMessage=""
          rejectPlaceholder="填写原因"
          request="审批视觉参照"
        />
      </div>
    )
    const panel = screen.container.querySelector<HTMLElement>('.human-interaction-panel')!
    const approval = screen.container.querySelector<HTMLElement>('.agent-approval-dialog')!
    const body = panel.querySelector<HTMLElement>('.human-interaction-panel__body')!
    const footer = panel.querySelector<HTMLElement>('footer')!
    const panelStyle = getComputedStyle(panel)
    const approvalStyle = getComputedStyle(approval)
    for (const property of [
      'fontSize',
      'lineHeight',
      'borderRadius',
      'backgroundColor',
      'padding'
    ] as const) {
      expect(panelStyle[property]).toBe(approvalStyle[property])
    }
    expect(getComputedStyle(panel.querySelector('h2')!).fontSize).toBe('13px')
    expect(panel.querySelector('h2')!.textContent).toBe('交互')
    expect(panel.querySelector('h2 .lucide-message-circle-question-mark')).not.toBeNull()
    expect(panel.querySelector('h3')!.textContent).toBe(title)
    expect(panel.querySelectorAll('.human-interaction-panel__option')).toHaveLength(12)
    expect(panel.scrollWidth).toBeLessThanOrEqual(320)
    expect(body.scrollWidth).toBeLessThanOrEqual(body.clientWidth)
    expect(body.scrollHeight).toBeGreaterThan(body.clientHeight)
    expect(footer.getBoundingClientRect().top).toBeGreaterThanOrEqual(
      body.getBoundingClientRect().bottom
    )
    expect(footer.getBoundingClientRect().right).toBeLessThanOrEqual(
      panel.getBoundingClientRect().right
    )
    await page.screenshot({
      element: panel,
      path: '.vitest-attachments/human-interaction-panel-long.png'
    })
    await screen.rerender(
      <div style={{ width: 440 }}>
        <Fixture initialAnswers={completed} sourceRunEnded />
      </div>
    )
    const compact = screen.container.querySelector<HTMLElement>('.human-interaction-panel')!
    const customRow = compact.querySelector<HTMLElement>('.human-interaction-panel__custom')!
    const labelBox = customRow.querySelector('label')!.getBoundingClientRect()
    const inputBox = customRow.querySelector('input')!.getBoundingClientRect()
    expect(inputBox.left).toBeGreaterThan(labelBox.right)
    expect(inputBox.height).toBe(28)
    expect(
      Math.abs((labelBox.top + labelBox.bottom) / 2 - (inputBox.top + inputBox.bottom) / 2)
    ).toBeLessThanOrEqual(1)
    const skip = screen.getByRole('button', { name: '跳过', exact: true })
    const skipColor = getComputedStyle(skip.element()).color
    expect(skipColor).toBe('rgb(26, 28, 31)')
    await skip.click()
    expect(
      getComputedStyle(screen.getByRole('button', { name: '已跳过', exact: true }).element()).color
    ).toBe(skipColor)
    expect(
      compact.querySelector('.human-interaction-panel__option')!.getBoundingClientRect().height
    ).toBe(28)
    expect(
      compact.querySelector('.human-interaction-panel__option-index')!.getBoundingClientRect()
        .height
    ).toBe(16)
    await page.screenshot({
      element: compact,
      path: '.vitest-attachments/human-interaction-panel.png'
    })
  })
})
