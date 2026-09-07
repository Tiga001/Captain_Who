const zhCN = {
  'humanInteraction.settings.title': '人机交互',
  'humanInteraction.settings.allowQuestions': '允许智能体向人类发起提问与协作',
  'humanInteraction.settings.description': '更改即时保存。关闭后，已提出的问题仍可回答或忽略。',
  'humanInteraction.settings.loading': '正在读取设置…',
  'humanInteraction.settings.saving': '正在保存…',
  'humanInteraction.settings.loadFailed': '无法读取人机交互设置，请重试。',
  'humanInteraction.settings.saveFailed': '未能保存更改，原设置已保留。',
  'humanInteraction.settings.retry': '重新读取'
} as const

type SettingsTranslations = Record<keyof typeof zhCN, string>

const enUS: SettingsTranslations = {
  'humanInteraction.settings.title': 'Human interaction',
  'humanInteraction.settings.allowQuestions':
    'Allow agents to ask questions and request human collaboration',
  'humanInteraction.settings.description':
    'Changes save immediately. Existing questions can still be answered or ignored when this is off.',
  'humanInteraction.settings.loading': 'Loading settings…',
  'humanInteraction.settings.saving': 'Saving…',
  'humanInteraction.settings.loadFailed':
    'Could not load human interaction settings. Please try again.',
  'humanInteraction.settings.saveFailed':
    'Could not save the change. The previous setting was kept.',
  'humanInteraction.settings.retry': 'Reload settings'
}

export const humanInteractionSettingsTranslations = {
  'zh-CN': zhCN,
  'zh-TW': {
    'humanInteraction.settings.title': '人機互動',
    'humanInteraction.settings.allowQuestions': '允許智能體向人類發起提問與協作',
    'humanInteraction.settings.description': '變更立即儲存。關閉後，已提出的問題仍可回答或忽略。',
    'humanInteraction.settings.loading': '正在讀取設定…',
    'humanInteraction.settings.saving': '正在儲存…',
    'humanInteraction.settings.loadFailed': '無法讀取人機互動設定，請重試。',
    'humanInteraction.settings.saveFailed': '無法儲存變更，已保留原設定。',
    'humanInteraction.settings.retry': '重新讀取'
  },
  'en-US': enUS,
  'en-GB': enUS,
  'ja-JP': {
    'humanInteraction.settings.title': 'ユーザーとの対話',
    'humanInteraction.settings.allowQuestions':
      'エージェントがユーザーに質問や協力を求めることを許可',
    'humanInteraction.settings.description':
      '変更はすぐに保存されます。オフにしても、既存の質問には回答または無視できます。',
    'humanInteraction.settings.loading': '設定を読み込み中…',
    'humanInteraction.settings.saving': '保存中…',
    'humanInteraction.settings.loadFailed':
      '対話設定を読み込めませんでした。もう一度お試しください。',
    'humanInteraction.settings.saveFailed':
      '変更を保存できませんでした。以前の設定が保持されています。',
    'humanInteraction.settings.retry': '設定を再読み込み'
  },
  'ko-KR': {
    'humanInteraction.settings.title': '사용자와의 상호작용',
    'humanInteraction.settings.allowQuestions':
      '에이전트가 사용자에게 질문하고 협력을 요청하도록 허용',
    'humanInteraction.settings.description':
      '변경 사항은 즉시 저장됩니다. 꺼도 기존 질문에 답하거나 무시할 수 있습니다.',
    'humanInteraction.settings.loading': '설정 불러오는 중…',
    'humanInteraction.settings.saving': '저장 중…',
    'humanInteraction.settings.loadFailed': '상호작용 설정을 불러오지 못했습니다. 다시 시도하세요.',
    'humanInteraction.settings.saveFailed':
      '변경 사항을 저장하지 못했습니다. 이전 설정이 유지됩니다.',
    'humanInteraction.settings.retry': '설정 다시 불러오기'
  },
  'fr-FR': {
    'humanInteraction.settings.title': 'Interaction humaine',
    'humanInteraction.settings.allowQuestions':
      'Autoriser les agents à poser des questions et à solliciter une collaboration humaine',
    'humanInteraction.settings.description':
      'Les changements sont enregistrés immédiatement. Les questions existantes peuvent toujours recevoir une réponse ou être ignorées.',
    'humanInteraction.settings.loading': 'Chargement des paramètres…',
    'humanInteraction.settings.saving': 'Enregistrement…',
    'humanInteraction.settings.loadFailed':
      'Impossible de charger les paramètres d’interaction. Veuillez réessayer.',
    'humanInteraction.settings.saveFailed':
      'Impossible d’enregistrer le changement. Le réglage précédent a été conservé.',
    'humanInteraction.settings.retry': 'Recharger les paramètres'
  },
  'it-IT': {
    'humanInteraction.settings.title': 'Interazione umana',
    'humanInteraction.settings.allowQuestions':
      'Consenti agli agenti di porre domande e richiedere la collaborazione umana',
    'humanInteraction.settings.description':
      'Le modifiche vengono salvate subito. Le domande esistenti possono ancora ricevere risposta o essere ignorate.',
    'humanInteraction.settings.loading': 'Caricamento delle impostazioni…',
    'humanInteraction.settings.saving': 'Salvataggio…',
    'humanInteraction.settings.loadFailed':
      'Impossibile caricare le impostazioni di interazione. Riprova.',
    'humanInteraction.settings.saveFailed':
      'Impossibile salvare la modifica. L’impostazione precedente è stata mantenuta.',
    'humanInteraction.settings.retry': 'Ricarica le impostazioni'
  },
  'ru-RU': {
    'humanInteraction.settings.title': 'Взаимодействие с человеком',
    'humanInteraction.settings.allowQuestions':
      'Разрешить агентам задавать вопросы людям и обращаться к ним за содействием',
    'humanInteraction.settings.description':
      'Изменения сохраняются сразу. На уже заданные вопросы по-прежнему можно ответить или проигнорировать их.',
    'humanInteraction.settings.loading': 'Загрузка настроек…',
    'humanInteraction.settings.saving': 'Сохранение…',
    'humanInteraction.settings.loadFailed':
      'Не удалось загрузить настройки взаимодействия. Повторите попытку.',
    'humanInteraction.settings.saveFailed':
      'Не удалось сохранить изменение. Предыдущая настройка сохранена.',
    'humanInteraction.settings.retry': 'Перезагрузить настройки'
  }
} as const satisfies Record<string, SettingsTranslations>
