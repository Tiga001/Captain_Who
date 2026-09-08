const zhCN = {
  'capabilityCenter.title': '能力中心',
  'capabilityCenter.description': '快捷开关软件能力与 MCP',
  'capabilityCenter.back': '返回命令列表',
  'capabilityCenter.search': '搜索能力',
  'capabilityCenter.image': '图片生成',
  'capabilityCenter.webSearch': '联网搜索',
  'capabilityCenter.human': '人机交互',
  'capabilityCenter.collaboration': '多智能体',
  'capabilityCenter.browser': '浏览器自动化',
  'capabilityCenter.notice': '能力中心',
  'capabilityCenter.acknowledge': '知道了',
  'capabilityCenter.imageNotConfigured':
    '图片生成尚未配置完整。请先在设置中配置图片生成服务与 API 密钥。',
  'capabilityCenter.searchNotConfigured': '联网搜索尚未配置。请先在设置中配置搜索服务的 API 密钥。',
  'capabilityCenter.mcpAuthorizationRequired':
    '此 MCP 尚未完成启动授权，或配置变更后需要重新授权。请在 MCP 设置中完成授权后再启用。',
  'capabilityCenter.saveFailed': '未能确认更改结果，已尝试重新读取当前状态。请稍后重试。',
  'capabilityCenter.loadFailed': '部分能力状态暂时无法读取。请稍后重新打开能力中心。',
  'capabilityCenter.noMcp': '尚未添加 MCP',
  'capabilityCenter.noResults': '没有匹配的能力'
} as const

type Catalog = Record<keyof typeof zhCN, string>
const enUS: Catalog = {
  'capabilityCenter.title': 'Capability center',
  'capabilityCenter.description': 'Quickly toggle app capabilities and MCP',
  'capabilityCenter.back': 'Back to commands',
  'capabilityCenter.search': 'Search capabilities',
  'capabilityCenter.image': 'Image generation',
  'capabilityCenter.webSearch': 'Web search',
  'capabilityCenter.human': 'Human interaction',
  'capabilityCenter.collaboration': 'Multiple agents',
  'capabilityCenter.browser': 'Browser automation',
  'capabilityCenter.notice': 'Capability center',
  'capabilityCenter.acknowledge': 'Got it',
  'capabilityCenter.imageNotConfigured':
    'Image generation is not fully configured. Configure the image service and its API key in Settings first.',
  'capabilityCenter.searchNotConfigured':
    'Web search is not configured. Configure the search service API key in Settings first.',
  'capabilityCenter.mcpAuthorizationRequired':
    'This MCP needs launch authorization, possibly because its configuration changed. Complete authorization in MCP settings before enabling it.',
  'capabilityCenter.saveFailed':
    'The change could not be confirmed. A refresh of the current state was attempted. Please try again later.',
  'capabilityCenter.loadFailed':
    'Some capability states could not be loaded. Please reopen the capability center later.',
  'capabilityCenter.noMcp': 'No MCP servers added',
  'capabilityCenter.noResults': 'No matching capabilities'
}

export const capabilityCenterTranslations = {
  'zh-CN': zhCN,
  'zh-TW': {
    'capabilityCenter.title': '能力中心',
    'capabilityCenter.description': '快速切換軟體能力與 MCP',
    'capabilityCenter.back': '返回命令清單',
    'capabilityCenter.search': '搜尋能力',
    'capabilityCenter.image': '圖片生成',
    'capabilityCenter.webSearch': '聯網搜尋',
    'capabilityCenter.human': '人機互動',
    'capabilityCenter.collaboration': '多智能體',
    'capabilityCenter.browser': '瀏覽器自動化',
    'capabilityCenter.notice': '能力中心',
    'capabilityCenter.acknowledge': '知道了',
    'capabilityCenter.imageNotConfigured':
      '圖片生成尚未設定完整。請先在設定中設定圖片生成服務與 API 金鑰。',
    'capabilityCenter.searchNotConfigured':
      '聯網搜尋尚未設定。請先在設定中設定搜尋服務的 API 金鑰。',
    'capabilityCenter.mcpAuthorizationRequired':
      '此 MCP 尚未完成啟動授權，或設定變更後需要重新授權。請在 MCP 設定中完成授權後再啟用。',
    'capabilityCenter.saveFailed': '無法確認變更結果，已嘗試重新讀取目前狀態。請稍後重試。',
    'capabilityCenter.loadFailed': '部分能力狀態暫時無法讀取。請稍後重新開啟能力中心。',
    'capabilityCenter.noMcp': '尚未新增 MCP',
    'capabilityCenter.noResults': '沒有符合的能力'
  },
  'en-US': enUS,
  'en-GB': enUS,
  'ja-JP': {
    'capabilityCenter.title': '機能センター',
    'capabilityCenter.description': 'アプリの機能と MCP をすばやく切り替え',
    'capabilityCenter.back': 'コマンド一覧に戻る',
    'capabilityCenter.search': '機能を検索',
    'capabilityCenter.image': '画像生成',
    'capabilityCenter.webSearch': 'ウェブ検索',
    'capabilityCenter.human': 'ユーザーとの対話',
    'capabilityCenter.collaboration': 'マルチエージェント',
    'capabilityCenter.browser': 'ブラウザー自動化',
    'capabilityCenter.notice': '機能センター',
    'capabilityCenter.acknowledge': '了解',
    'capabilityCenter.imageNotConfigured':
      '画像生成の設定が完了していません。設定で画像生成サービスと API キーを設定してください。',
    'capabilityCenter.searchNotConfigured':
      'ウェブ検索が未設定です。設定で検索サービスの API キーを設定してください。',
    'capabilityCenter.mcpAuthorizationRequired':
      'この MCP は起動の承認が必要です。設定の変更によって再承認が必要になる場合もあります。MCP 設定で承認してから有効にしてください。',
    'capabilityCenter.saveFailed':
      '変更結果を確認できなかったため、現在の状態の再読み込みを試みました。後でもう一度お試しください。',
    'capabilityCenter.loadFailed':
      '一部の機能の状態を読み込めません。後でもう一度機能センターを開いてください。',
    'capabilityCenter.noMcp': 'MCP サーバーは未登録です',
    'capabilityCenter.noResults': '一致する機能はありません'
  },
  'ko-KR': {
    'capabilityCenter.title': '기능 센터',
    'capabilityCenter.description': '앱 기능 및 MCP 빠르게 전환',
    'capabilityCenter.back': '명령 목록으로 돌아가기',
    'capabilityCenter.search': '기능 검색',
    'capabilityCenter.image': '이미지 생성',
    'capabilityCenter.webSearch': '웹 검색',
    'capabilityCenter.human': '사용자 상호작용',
    'capabilityCenter.collaboration': '다중 에이전트',
    'capabilityCenter.browser': '브라우저 자동화',
    'capabilityCenter.notice': '기능 센터',
    'capabilityCenter.acknowledge': '확인',
    'capabilityCenter.imageNotConfigured':
      '이미지 생성 설정이 완료되지 않았습니다. 설정에서 이미지 서비스와 API 키를 먼저 구성하세요.',
    'capabilityCenter.searchNotConfigured':
      '웹 검색이 구성되지 않았습니다. 설정에서 검색 서비스 API 키를 먼저 구성하세요.',
    'capabilityCenter.mcpAuthorizationRequired':
      '이 MCP에는 시작 승인이 필요합니다. 구성이 변경되면 다시 승인해야 할 수 있습니다. MCP 설정에서 승인한 후 활성화하세요.',
    'capabilityCenter.saveFailed':
      '변경 결과를 확인할 수 없어 현재 상태를 다시 읽으려고 했습니다. 나중에 다시 시도하세요.',
    'capabilityCenter.loadFailed':
      '일부 기능 상태를 읽을 수 없습니다. 나중에 기능 센터를 다시 여세요.',
    'capabilityCenter.noMcp': '추가된 MCP 서버 없음',
    'capabilityCenter.noResults': '일치하는 기능 없음'
  },
  'fr-FR': {
    'capabilityCenter.title': 'Centre de fonctionnalités',
    'capabilityCenter.description': 'Activer ou désactiver les fonctionnalités et MCP',
    'capabilityCenter.back': 'Retour aux commandes',
    'capabilityCenter.search': 'Rechercher une fonctionnalité',
    'capabilityCenter.image': 'Génération d’images',
    'capabilityCenter.webSearch': 'Recherche web',
    'capabilityCenter.human': 'Interaction humaine',
    'capabilityCenter.collaboration': 'Agents multiples',
    'capabilityCenter.browser': 'Automatisation du navigateur',
    'capabilityCenter.notice': 'Centre de fonctionnalités',
    'capabilityCenter.acknowledge': 'Compris',
    'capabilityCenter.imageNotConfigured':
      'La génération d’images n’est pas entièrement configurée. Configurez le service et sa clé API dans les paramètres.',
    'capabilityCenter.searchNotConfigured':
      'La recherche web n’est pas configurée. Configurez la clé API du service dans les paramètres.',
    'capabilityCenter.mcpAuthorizationRequired':
      'Ce MCP nécessite une autorisation de lancement, éventuellement après un changement de configuration. Autorisez-le dans les paramètres MCP avant de l’activer.',
    'capabilityCenter.saveFailed':
      'La modification n’a pas pu être confirmée. Une actualisation de l’état a été tentée. Réessayez plus tard.',
    'capabilityCenter.loadFailed':
      'Certains états n’ont pas pu être chargés. Rouvrez le centre de fonctionnalités plus tard.',
    'capabilityCenter.noMcp': 'Aucun serveur MCP ajouté',
    'capabilityCenter.noResults': 'Aucune fonctionnalité correspondante'
  },
  'it-IT': {
    'capabilityCenter.title': 'Centro funzionalità',
    'capabilityCenter.description': 'Attiva o disattiva funzionalità e MCP',
    'capabilityCenter.back': 'Torna ai comandi',
    'capabilityCenter.search': 'Cerca funzionalità',
    'capabilityCenter.image': 'Generazione immagini',
    'capabilityCenter.webSearch': 'Ricerca web',
    'capabilityCenter.human': 'Interazione umana',
    'capabilityCenter.collaboration': 'Agenti multipli',
    'capabilityCenter.browser': 'Automazione del browser',
    'capabilityCenter.notice': 'Centro funzionalità',
    'capabilityCenter.acknowledge': 'Capito',
    'capabilityCenter.imageNotConfigured':
      'La generazione di immagini non è completamente configurata. Configura il servizio e la chiave API nelle impostazioni.',
    'capabilityCenter.searchNotConfigured':
      'La ricerca web non è configurata. Configura la chiave API del servizio nelle impostazioni.',
    'capabilityCenter.mcpAuthorizationRequired':
      'Questo MCP richiede un’autorizzazione di avvio, eventualmente dopo una modifica alla configurazione. Autorizzalo nelle impostazioni MCP prima di attivarlo.',
    'capabilityCenter.saveFailed':
      'Non è stato possibile confermare la modifica. È stato tentato un aggiornamento dello stato. Riprova più tardi.',
    'capabilityCenter.loadFailed':
      'Impossibile caricare alcuni stati. Riapri il centro funzionalità più tardi.',
    'capabilityCenter.noMcp': 'Nessun server MCP aggiunto',
    'capabilityCenter.noResults': 'Nessuna funzionalità corrispondente'
  },
  'ru-RU': {
    'capabilityCenter.title': 'Центр возможностей',
    'capabilityCenter.description': 'Быстрое включение возможностей и MCP',
    'capabilityCenter.back': 'Назад к командам',
    'capabilityCenter.search': 'Поиск возможностей',
    'capabilityCenter.image': 'Генерация изображений',
    'capabilityCenter.webSearch': 'Поиск в интернете',
    'capabilityCenter.human': 'Взаимодействие с человеком',
    'capabilityCenter.collaboration': 'Несколько агентов',
    'capabilityCenter.browser': 'Автоматизация браузера',
    'capabilityCenter.notice': 'Центр возможностей',
    'capabilityCenter.acknowledge': 'Понятно',
    'capabilityCenter.imageNotConfigured':
      'Генерация изображений настроена не полностью. Сначала настройте сервис и ключ API в настройках.',
    'capabilityCenter.searchNotConfigured':
      'Поиск в интернете не настроен. Сначала укажите ключ API поискового сервиса в настройках.',
    'capabilityCenter.mcpAuthorizationRequired':
      'Для этого MCP нужно разрешить запуск. После изменения конфигурации разрешение может потребоваться снова. Выдайте разрешение в настройках MCP перед включением.',
    'capabilityCenter.saveFailed':
      'Не удалось подтвердить изменение. Выполнена попытка обновить текущее состояние. Повторите позже.',
    'capabilityCenter.loadFailed':
      'Не удалось загрузить состояние некоторых возможностей. Откройте центр возможностей позже.',
    'capabilityCenter.noMcp': 'Серверы MCP не добавлены',
    'capabilityCenter.noResults': 'Совпадений нет'
  }
} satisfies Record<string, Catalog>
