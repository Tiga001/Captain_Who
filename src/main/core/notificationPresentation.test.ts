import { describe, expect, it } from 'vitest'
import type { NotificationBatch } from '@mycopilot/protocol'
import type { AppLanguage } from '../../shared/i18n/languageRegistry'
import { formatNotificationBatch } from '../notifications/notificationPresentation'

const EXPECTED_COMPLETED_PRESENTATIONS = {
  'zh-CN': {
    title: '任务已完成',
    body: '“Prepare a detailed unit report”已完成。',
    hiddenBody: '打开软件查看运行结果。'
  },
  'zh-TW': {
    title: '任務已完成',
    body: '「Prepare a detailed unit report」已完成。',
    hiddenBody: '開啟軟體查看執行結果。'
  },
  'en-US': {
    title: 'Task completed',
    body: '“Prepare a detailed unit report” completed.',
    hiddenBody: 'Open the app to view the result.'
  },
  'en-GB': {
    title: 'Task completed',
    body: '“Prepare a detailed unit report” completed.',
    hiddenBody: 'Open the app to view the result.'
  },
  'ko-KR': {
    title: '작업 완료',
    body: '“Prepare a detailed unit report” 작업이 완료되었습니다.',
    hiddenBody: '앱을 열어 결과를 확인하세요.'
  },
  'ja-JP': {
    title: 'タスクが完了しました',
    body: '「Prepare a detailed unit report」が完了しました。',
    hiddenBody: 'アプリを開いて結果を確認してください。'
  },
  'fr-FR': {
    title: 'Tâche terminée',
    body: 'Tâche terminée : « Prepare a detailed unit report ».',
    hiddenBody: 'Ouvrez l’application pour voir le résultat.'
  },
  'it-IT': {
    title: 'Attività completata',
    body: 'Attività “Prepare a detailed unit report” completata.',
    hiddenBody: 'Apri l’app per vedere il risultato.'
  },
  'ru-RU': {
    title: 'Задача завершена',
    body: 'Задача «Prepare a detailed unit report» завершена.',
    hiddenBody: 'Откройте приложение, чтобы посмотреть результат.'
  }
} as const satisfies Record<
  AppLanguage,
  { readonly title: string; readonly body: string; readonly hiddenBody: string }
>

const EXPECTED_HIDDEN_AUTOMATION_TITLES = {
  'zh-CN': '自动化任务发现重要更新',
  'zh-TW': '自動化任務發現重要更新',
  'en-US': 'Scheduled task found an important update',
  'en-GB': 'Scheduled task found an important update',
  'ko-KR': '예약 작업: 중요 업데이트 발견',
  'ja-JP': 'スケジュールされたタスクに重要な更新があります',
  'fr-FR': 'Mise à jour importante : Tâche planifiée',
  'it-IT': 'Aggiornamento importante: Attività pianificata',
  'ru-RU': 'Важное обновление: Запланированная задача'
} as const satisfies Record<AppLanguage, string>

const EXPECTED_BATCH_PRESENTATIONS = {
  'zh-CN': {
    title: '4 项任务有新状态',
    body: '1 项需要批准 · 1 项失败 · 2 项已完成'
  },
  'zh-TW': {
    title: '4 項任務有新狀態',
    body: '1 項需要批准 · 1 項失敗 · 2 項已完成'
  },
  'en-US': {
    title: '4 task updates',
    body: '1 need approval · 1 failed · 2 completed'
  },
  'en-GB': {
    title: '4 task updates',
    body: '1 need approval · 1 failed · 2 completed'
  },
  'ko-KR': {
    title: '작업 업데이트 4건',
    body: '승인 필요 1건 · 실패 1건 · 완료 2건'
  },
  'ja-JP': {
    title: '4件のタスクに更新があります',
    body: '承認待ち 1件 · 失敗 1件 · 完了 2件'
  },
  'fr-FR': {
    title: '4 mises à jour de tâches',
    body: 'À approuver : 1 · Échecs : 1 · Terminées : 2'
  },
  'it-IT': {
    title: '4 aggiornamenti delle attività',
    body: 'Da approvare: 1 · Non riuscite: 1 · Completate: 2'
  },
  'ru-RU': {
    title: 'Обновления задач: 4',
    body: 'Требуют одобрения: 1 · С ошибкой: 1 · Завершено: 2'
  }
} as const satisfies Record<AppLanguage, { readonly title: string; readonly body: string }>

function singleBatch(overrides: Partial<NotificationBatch> = {}): NotificationBatch {
  return {
    schemaVersion: 1,
    batchId: 'batch-1',
    revision: 1,
    status: 'claimed',
    highestPriority: 'completed',
    counts: {
      completed: 1,
      failed: 0,
      cancelled: 0,
      approvalRequired: 0,
      importantUpdate: 0,
      configurationBlocked: 0
    },
    itemCount: 1,
    items: [
      {
        schemaVersion: 1,
        eventId: 'event-1',
        batchId: 'batch-1',
        kind: 'task_completed',
        sourceKind: 'human_root',
        sourceId: 'run-1',
        runId: 'run-1',
        automationId: null,
        conversationId: 'conversation-1',
        userMessageId: 'message-1',
        assistantMessageId: 'message-2',
        approvalActionId: null,
        subjectKind: 'prompt_excerpt',
        subjectText: 'Prepare a detailed unit report',
        priority: 'completed',
        resourceRevision: 1,
        seenAt: null,
        resolvedAt: null,
        occurredAt: 100
      }
    ],
    collectUntil: 100,
    replaceUntil: 60_000,
    notificationsEnabled: true,
    soundEnabled: true,
    showTaskContent: true,
    soundLevelPlayed: 'none',
    deliveredRevision: null,
    deliveredPriority: null,
    isUpdate: false,
    createdAt: 100,
    updatedAt: 100,
    ...overrides
  }
}

describe('system notification presentation', () => {
  it('uses only the frozen prompt excerpt for a HumanRoot task', () => {
    expect(formatNotificationBatch(singleBatch(), 'zh-CN')).toEqual({
      title: '任务已完成',
      body: '“Prepare a detailed unit report”已完成。'
    })
  })

  it('hides the prompt when notification previews are disabled', () => {
    expect(formatNotificationBatch(singleBatch({ showTaskContent: false }), 'zh-CN')).toEqual({
      title: '任务已完成',
      body: '打开软件查看运行结果。'
    })
  })

  it('formats task subjects and hidden previews in every registered application language', () => {
    for (const language of Object.keys(EXPECTED_COMPLETED_PRESENTATIONS) as AppLanguage[]) {
      const expected = EXPECTED_COMPLETED_PRESENTATIONS[language]
      expect(formatNotificationBatch(singleBatch(), language), language).toEqual({
        title: expected.title,
        body: expected.body
      })
      expect(
        formatNotificationBatch(singleBatch({ showTaskContent: false }), language),
        `${language}:hidden`
      ).toEqual({
        title: expected.title,
        body: expected.hiddenBody
      })
    }
  })

  it.each([
    ['task_failed', 'Échec de la tâche', 'Ouvrez l’application pour voir les détails de l’échec.'],
    ['task_cancelled', 'Tâche annulée', 'La tâche a été annulée.'],
    [
      'approval_required',
      'Votre approbation est requise',
      'Ouvrez l’application pour examiner l’action en attente d’approbation.'
    ]
  ] as const)('localizes the hidden %s presentation', (kind, title, body) => {
    const value = singleBatch({ showTaskContent: false })
    value.items[0] = { ...value.items[0], kind }
    expect(formatNotificationBatch(value, 'fr-FR')).toEqual({ title, body })
  })

  it('localizes the attachment-only fallback instead of exposing a stored locale', () => {
    const value = singleBatch()
    value.items[0] = {
      ...value.items[0],
      subjectKind: 'attachment_task',
      subjectText: '附件任务'
    }
    expect(formatNotificationBatch(value, 'en-US')).toEqual({
      title: 'Task completed',
      body: '“Attachment task” completed.'
    })
  })

  it('uses only the frozen automation title and never adds a model-generated body', () => {
    const value = singleBatch()
    value.items[0] = {
      ...value.items[0],
      kind: 'automation_important_update',
      sourceKind: 'automation',
      sourceId: 'automation-1',
      automationId: 'automation-1',
      subjectKind: 'automation_title',
      subjectText: '每日简报',
      priority: 'important_update'
    }
    value.highestPriority = 'important_update'
    value.counts.completed = 0
    value.counts.importantUpdate = 1

    expect(formatNotificationBatch(value, 'zh-CN')).toEqual({
      title: '每日简报发现重要更新',
      body: ''
    })
  })

  it('hides the automation title in both supported presentation languages', () => {
    const value = singleBatch({ showTaskContent: false })
    value.items[0] = {
      ...value.items[0],
      kind: 'automation_important_update',
      sourceKind: 'automation',
      sourceId: 'automation-1',
      automationId: 'automation-1',
      subjectKind: 'automation_title',
      subjectText: 'Private customer monitor',
      priority: 'important_update'
    }
    value.highestPriority = 'important_update'
    value.counts.completed = 0
    value.counts.importantUpdate = 1

    expect(formatNotificationBatch(value, 'zh-CN')).toEqual({
      title: '自动化任务发现重要更新',
      body: ''
    })
    expect(formatNotificationBatch(value, 'en-US')).toEqual({
      title: 'Scheduled task found an important update',
      body: ''
    })
  })

  it('uses the localized automation fallback in every application language', () => {
    const value = singleBatch({ showTaskContent: false })
    value.items[0] = {
      ...value.items[0],
      kind: 'automation_important_update',
      sourceKind: 'automation',
      sourceId: 'automation-1',
      automationId: 'automation-1',
      subjectKind: 'automation_title',
      subjectText: 'Private customer monitor',
      priority: 'important_update'
    }
    value.highestPriority = 'important_update'
    value.counts.completed = 0
    value.counts.importantUpdate = 1

    for (const language of Object.keys(EXPECTED_HIDDEN_AUTOMATION_TITLES) as AppLanguage[]) {
      expect(formatNotificationBatch(value, language), language).toEqual({
        title: EXPECTED_HIDDEN_AUTOMATION_TITLES[language],
        body: ''
      })
    }
  })

  it.each([
    ['automation_completed', 'Esecuzione completata: Daily digest'],
    ['automation_failed', 'Esecuzione non riuscita: Daily digest'],
    ['automation_cancelled', 'Esecuzione annullata: Daily digest'],
    ['approval_required', 'Approvazione richiesta: Daily digest'],
    ['automation_important_update', 'Aggiornamento importante: Daily digest'],
    ['automation_configuration_blocked', 'Intervento richiesto: Daily digest']
  ] as const)('localizes the %s automation title', (kind, title) => {
    const value = singleBatch()
    value.items[0] = {
      ...value.items[0],
      kind,
      sourceKind: 'automation',
      sourceId: 'automation-1',
      automationId: 'automation-1',
      subjectKind: 'automation_title',
      subjectText: 'Daily digest'
    }
    expect(formatNotificationBatch(value, 'it-IT')).toEqual({ title, body: '' })
  })

  it('localizes merged status counts in every application language', () => {
    const value = singleBatch({
      itemCount: 4,
      counts: {
        completed: 2,
        failed: 1,
        cancelled: 0,
        approvalRequired: 1,
        importantUpdate: 0,
        configurationBlocked: 0
      }
    })

    for (const language of Object.keys(EXPECTED_BATCH_PRESENTATIONS) as AppLanguage[]) {
      expect(formatNotificationBatch(value, language), language).toEqual(
        EXPECTED_BATCH_PRESENTATIONS[language]
      )
    }
  })

  it('includes every merged status from localized count templates', () => {
    const value = singleBatch({
      itemCount: 6,
      counts: {
        completed: 1,
        failed: 1,
        cancelled: 1,
        approvalRequired: 1,
        importantUpdate: 1,
        configurationBlocked: 1
      }
    })

    expect(formatNotificationBatch(value, 'ja-JP')).toEqual({
      title: '6件のタスクに更新があります',
      body: '承認待ち 1件 · 要修正 1件 · 失敗 1件 · 重要な更新 1件 · キャンセル 1件 · 完了 1件'
    })
  })

  it('uses the localized all-completed batch title', () => {
    const value = singleBatch({
      itemCount: 2,
      counts: {
        completed: 2,
        failed: 0,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      }
    })

    expect(formatNotificationBatch(value, 'ru-RU')).toEqual({
      title: 'Завершённые задачи: 2',
      body: 'Завершено: 2'
    })
  })

  it('truncates long subjects by grapheme without exposing the remaining prompt', () => {
    const value = singleBatch()
    value.items[0] = {
      ...value.items[0],
      subjectText: '👩🏽\u200d💻'.repeat(60)
    }
    const presentation = formatNotificationBatch(value, 'en-US')
    const emojiCount = [...presentation.body.matchAll(/👩🏽\u200d💻/gu)].length
    expect(emojiCount).toBe(48)
    expect(presentation.body.endsWith('…” completed.')).toBe(true)
  })
})
