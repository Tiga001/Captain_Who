const zhCN = {
  'settings.search.clear': '清空搜索',
  'settings.search.results': '设置搜索结果',
  'settings.search.empty': '未找到匹配的设置',
  'settings.search.emptyHint': '试试设置名称或说明中的关键词',
  'settings.search.context': '需要先选择对象或启用相关设置',
  'settings.search.waiting': '正在定位设置…'
} as const
type Messages = Record<keyof typeof zhCN, string>
const en: Messages = {
  'settings.search.clear': 'Clear search',
  'settings.search.results': 'Settings search results',
  'settings.search.empty': 'No matching settings',
  'settings.search.emptyHint': 'Try a word from a setting name or description',
  'settings.search.context': 'Select an item or enable the related setting first',
  'settings.search.waiting': 'Locating setting…'
}
function messages(values: [string, string, string, string, string, string]): Messages {
  return Object.fromEntries(Object.keys(zhCN).map((key, index) => [key, values[index]])) as Messages
}
export const settingsSearchTranslations = {
  'zh-CN': zhCN,
  'zh-TW': messages([
    '清除搜尋',
    '設定搜尋結果',
    '找不到符合的設定',
    '試試設定名稱或說明中的關鍵字',
    '請先選擇項目或啟用相關設定',
    '正在定位設定…'
  ]),
  'en-US': en,
  'en-GB': en,
  'ja-JP': messages([
    '検索をクリア',
    '設定の検索結果',
    '一致する設定がありません',
    '設定名や説明のキーワードを試してください',
    '先に項目を選択するか、関連設定を有効にしてください',
    '設定を表示中…'
  ]),
  'ko-KR': messages([
    '검색 지우기',
    '설정 검색 결과',
    '일치하는 설정이 없습니다',
    '설정 이름이나 설명의 키워드를 사용해 보세요',
    '먼저 항목을 선택하거나 관련 설정을 켜세요',
    '설정 찾는 중…'
  ]),
  'fr-FR': messages([
    'Effacer la recherche',
    'Résultats des paramètres',
    'Aucun paramètre correspondant',
    'Essayez un mot du nom ou de la description',
    'Sélectionnez un élément ou activez le réglage associé',
    'Localisation du paramètre…'
  ]),
  'it-IT': messages([
    'Cancella ricerca',
    'Risultati delle impostazioni',
    'Nessuna impostazione corrispondente',
    'Prova una parola del nome o della descrizione',
    'Seleziona prima un elemento o attiva la relativa impostazione',
    'Individuazione dell’impostazione…'
  ]),
  'ru-RU': messages([
    'Очистить поиск',
    'Результаты поиска настроек',
    'Настройки не найдены',
    'Введите слово из названия или описания настройки',
    'Сначала выберите элемент или включите связанную настройку',
    'Поиск расположения настройки…'
  ])
} as const
