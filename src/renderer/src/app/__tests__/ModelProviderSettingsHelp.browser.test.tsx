import { describe, expect, it, vi } from 'vitest'
import { userEvent } from 'vitest/browser'
import { render } from 'vitest-browser-react'

const translations = vi.hoisted(() => ({
  'configuration.model': '对话模型',
  'configuration.modelSettings': '模型配置',
  'configuration.modelSettingsHelp.open': '查看配置说明',
  'configuration.modelSettingsHelp.title': '默认接口配置',
  'configuration.modelSettingsHelp.description':
    '这里设置模型默认使用的 API 地址和 Token。模型未单独配置时会使用这里的值；模型内部填写了对应配置时，则优先使用模型自己的配置。',
  'configuration.modelSettingsHelp.note': '修改这里的配置，只会影响仍在使用默认配置的模型。',
  'configuration.modelSettingsHelp.acknowledge': '知道了',
  'configuration.modelSettingsHelp.close': '关闭默认接口配置',
  'configuration.availableModels': '可用模型',
  'configuration.manageModels': '管理模型',
  'configuration.availableModelList': '可用模型列表'
}))

vi.mock('../../config/FrontendConfigProvider', () => ({
  useFrontendConfig: () => ({
    t: (key: keyof typeof translations) => translations[key] ?? key
  })
}))

const { ModelProviderSettings } =
  await import('../../features/settings/pages/configuration/ModelProviderSettings')

function renderSettings(onApiUrlChange = vi.fn().mockResolvedValue(undefined)) {
  return render(
    <ModelProviderSettings
      apiTokenStatus="missing"
      apiUrl=""
      models={[]}
      onApiTokenCommit={vi.fn()}
      onApiUrlChange={onApiUrlChange}
      onManageModels={vi.fn()}
      onToggleModel={vi.fn()}
    />
  )
}

describe('ModelProviderSettings default API help', () => {
  it('keeps URL keystrokes local and commits only on blur or Enter', async () => {
    const onApiUrlChange = vi.fn().mockResolvedValue(undefined)
    const screen = await renderSettings(onApiUrlChange)
    const input = screen.getByRole('textbox', { name: 'API URL' })

    await input.fill('https://provider.example/v1')
    expect(onApiUrlChange).not.toHaveBeenCalled()
    await input.click()
    await userEvent.keyboard('{Enter}')
    await expect.poll(() => onApiUrlChange.mock.calls).toEqual([['https://provider.example/v1']])

    await input.fill('https://second.example/v1')
    expect(onApiUrlChange).toHaveBeenCalledTimes(1)
    await screen.getByRole('heading', { name: '模型配置' }).click()
    await expect
      .poll(() => onApiUrlChange.mock.calls)
      .toEqual([['https://provider.example/v1'], ['https://second.example/v1']])
  })

  it('is hidden by default and exposes an accessible compact help trigger', async () => {
    const screen = await renderSettings()
    const trigger = screen.getByRole('button', { name: '查看配置说明' })

    await expect.element(trigger).toHaveAttribute('title', '查看配置说明')
    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
  })

  it('opens the explanation and closes it with the single acknowledgement action', async () => {
    const screen = await renderSettings()
    const trigger = screen.getByRole('button', { name: '查看配置说明' })
    await trigger.click()

    const dialog = screen.getByRole('dialog', { name: '默认接口配置' })
    await expect.element(dialog).toBeVisible()
    await expect
      .element(
        screen.getByText(
          '这里设置模型默认使用的 API 地址和 Token。模型未单独配置时会使用这里的值；模型内部填写了对应配置时，则优先使用模型自己的配置。 修改这里的配置，只会影响仍在使用默认配置的模型。'
        )
      )
      .toBeVisible()

    const acknowledge = screen.getByRole('button', { name: '知道了' })
    await expect.poll(() => document.activeElement).toBe(acknowledge.element())
    await acknowledge.click()

    await expect.element(dialog).not.toBeInTheDocument()
    await expect.element(trigger).toHaveFocus()
  })

  it('closes with Escape and restores focus to the help trigger', async () => {
    const screen = await renderSettings()
    const trigger = screen.getByRole('button', { name: '查看配置说明' })
    await trigger.click()
    await userEvent.keyboard('{Escape}')

    await expect.element(screen.getByRole('dialog')).not.toBeInTheDocument()
    await expect.element(trigger).toHaveFocus()
  })
})
