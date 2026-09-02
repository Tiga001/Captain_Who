// Renderer startup tests: verify ambient phrase selection never repeats the current phrase.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { zhCNTranslations } from '../../config/frontendTranslations.zhCN'
import {
  getStartupCharacterIntervalMs,
  pickFirstStartupPhraseIndex,
  pickNextStartupPhraseIndex,
  pickStartupPhraseGroupIndex,
  STARTUP_AMBIENT_PHRASE_GROUPS,
  STARTUP_BOOTSTRAP_PHRASE_KEY
} from '../../features/startup/startupAmbientPhrases'

describe('pickNextStartupPhraseIndex', () => {
  it('keeps a localizable startup shell before React mounts', () => {
    const rendererShell = readFileSync(resolve('src/renderer/index.html'), 'utf8')
    expect(rendererShell).toContain('class="app-bootstrap-screen__ambient"')
    expect(rendererShell).toContain('data-bootstrap-startup-ambient')
    expect(rendererShell).toContain('data-bootstrap-startup-label')
    expect(rendererShell).toContain('data-bootstrap-startup-icon')
    expect(rendererShell).not.toContain('../../resources/brand-mark-light.png')
    expect(rendererShell).not.toContain('../../resources/brand-mark-dark.png')
    expect(rendererShell).toContain('/src/features/startup/bootstrapStartupEntry.ts')
    expect(rendererShell.indexOf('/src/features/startup/bootstrapStartupEntry.ts')).toBeLessThan(
      rendererShell.indexOf('/src/main.tsx')
    )
    expect(rendererShell).not.toContain('>正在深度思考<')
    expect(rendererShell).not.toContain('aria-label="Starting Captain Who"')
    expect(rendererShell).toContain('background: rgba(244, 244, 242, 0.58)')
    expect(rendererShell).toContain("[data-phase='fading']")
    expect(rendererShell).toContain('@media (prefers-reduced-transparency: reduce)')
  })

  it('keeps the full application behind the Host readiness gate', () => {
    const rendererEntry = readFileSync(resolve('src/renderer/src/main.tsx'), 'utf8')
    expect(rendererEntry).toContain('void hostClient.app')
    expect(rendererEntry).toContain('.whenReady()')
    expect(rendererEntry).toContain("import('./App')")
    expect(rendererEntry).not.toContain("import App from './App'")
    expect(rendererEntry).toContain('.catch(() => failBootstrapStartup())')
    expect(rendererEntry.indexOf('.whenReady()')).toBeLessThan(
      rendererEntry.indexOf("import('./App')")
    )
  })

  it('resolves the existing brand artwork through the Renderer asset pipeline', () => {
    const bootstrapEntry = readFileSync(
      resolve('src/renderer/src/features/startup/bootstrapStartupEntry.ts'),
      'utf8'
    )
    expect(bootstrapEntry).toContain("resources/brand-mark-light.png'")
    expect(bootstrapEntry).toContain("resources/brand-mark-dark.png'")
    expect(bootstrapEntry).toContain('brandImage.src = lightBrandMark')
    expect(bootstrapEntry).toContain('darkBrandSource.srcset = darkBrandMark')
  })

  it('returns the only phrase when the catalog has fewer than two entries', () => {
    expect(pickNextStartupPhraseIndex(0, 0, () => 0.75)).toBe(0)
    expect(pickNextStartupPhraseIndex(0, 1, () => 0.75)).toBe(0)
  })

  it('selects one phrase group for a startup session', () => {
    expect(pickStartupPhraseGroupIndex(3, () => 0)).toBe(0)
    expect(pickStartupPhraseGroupIndex(3, () => 0.99)).toBe(2)
  })

  it('keeps the original, reflective, and LLM catalogs as separate groups', () => {
    expect(STARTUP_AMBIENT_PHRASE_GROUPS).toHaveLength(3)
    expect(STARTUP_AMBIENT_PHRASE_GROUPS[0]).toHaveLength(11)
    expect(STARTUP_AMBIENT_PHRASE_GROUPS[0].map((key) => zhCNTranslations[key])).toContain(
      '正在连接 MCP 工具'
    )
    expect(STARTUP_AMBIENT_PHRASE_GROUPS[1].map((key) => zhCNTranslations[key])).toEqual([
      'token正在寻找它的位置',
      '让灵感在秩序中生长',
      '在混沌中辨认方向',
      '为模糊的想法赋予结构',
      '沿着问题慢慢接近答案',
      '与AI共同凝视问题',
      '在细节深处发现新的可能',
      '把想象交给严谨去实现',
      '世界尚未完成，创造仍在继续',
      '一个协议，连接无数工具',
      'Attention is all you need'
    ])
    expect(STARTUP_AMBIENT_PHRASE_GROUPS[2].map((key) => zhCNTranslations[key])).toEqual([
      '正在预填充上下文',
      '正在加载 KV Cache',
      'Token 正在自回归',
      '因果掩码已经落下',
      'RoPE 正在旋转位置',
      'Softmax 正在分配注意力',
      '多头注意力正在分工',
      'Logits 正在重排可能性',
      '正在采样下一个 Token',
      '稀疏专家正在路由',
      '残差流仍在继续',
      '梯度正在反向穿行',
      '随机梯度正在下降',
      '正在穿越损失曲面',
      '正在执行监督微调',
      '偏好模型正在打分',
      '奖励信号正在回传',
      '正在进行强化学习对齐',
      'LoRA 正在低秩适配',
      '权重正在量化',
      '张量正在跨设备并行',
      '前缀缓存已经命中',
      '上下文窗口正在压缩',
      '正在校准不确定性',
      '注意力不是免费的',
      '长上下文不等于长记忆',
      'Loss 下降，问题未必消失',
      '专家路由拒绝平均主义',
      '温度正在决定想象力',
      '熵仍在可控范围内',
      '参数很多，答案只有一个',
      '涌现正在等待规模',
      '梯度知道来时的路',
      'Attention 仍然是 all you need',
      'MCP Server 正在握手',
      'JSON-RPC 正在交换意图'
    ])
  })

  it('types English phrases faster than Chinese phrases', () => {
    expect(getStartupCharacterIntervalMs('zh-CN')).toBe(74)
    expect(getStartupCharacterIntervalMs('en-US')).toBe(52)
  })

  it('does not immediately replay the bootstrap phrase when its group is selected', () => {
    expect(
      pickFirstStartupPhraseIndex(
        STARTUP_AMBIENT_PHRASE_GROUPS[0],
        STARTUP_BOOTSTRAP_PHRASE_KEY,
        () => 0
      )
    ).toBe(1)
  })

  it('maps random slots around the current phrase without repeating it', () => {
    expect(pickNextStartupPhraseIndex(0, 4, () => 0)).toBe(1)
    expect(pickNextStartupPhraseIndex(1, 4, () => 0)).toBe(0)
    expect(pickNextStartupPhraseIndex(1, 4, () => 0.99)).toBe(3)
    expect(pickNextStartupPhraseIndex(3, 4, () => 0.99)).toBe(2)
  })
})
