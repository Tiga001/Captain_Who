export type WorkspaceMarkdownView = 'preview' | 'source'

export type WorkspaceOfficeDocumentType = 'Excel' | 'PowerPoint' | 'Word'

const MARKDOWN_EXTENSIONS = new Set(['.markdown', '.md', '.mdown', '.mdwn', '.mkd', '.mkdn'])

const OFFICE_DOCUMENT_TYPE_BY_EXTENSION: Readonly<Record<string, WorkspaceOfficeDocumentType>> = {
  '.doc': 'Word',
  '.docm': 'Word',
  '.docx': 'Word',
  '.dot': 'Word',
  '.dotm': 'Word',
  '.dotx': 'Word',
  '.rtf': 'Word',
  '.xls': 'Excel',
  '.xlsb': 'Excel',
  '.xlsm': 'Excel',
  '.xlsx': 'Excel',
  '.xlt': 'Excel',
  '.xltm': 'Excel',
  '.xltx': 'Excel',
  '.pot': 'PowerPoint',
  '.potm': 'PowerPoint',
  '.potx': 'PowerPoint',
  '.pps': 'PowerPoint',
  '.ppsm': 'PowerPoint',
  '.ppsx': 'PowerPoint',
  '.ppt': 'PowerPoint',
  '.pptm': 'PowerPoint',
  '.pptx': 'PowerPoint'
}

export function isWorkspaceMarkdownFile(path: string | null): boolean {
  return path ? MARKDOWN_EXTENSIONS.has(getFileExtension(path)) : false
}

export function getWorkspaceOfficeDocumentType(
  path: string | null
): WorkspaceOfficeDocumentType | null {
  return path ? (OFFICE_DOCUMENT_TYPE_BY_EXTENSION[getFileExtension(path)] ?? null) : null
}

function getFileExtension(path: string): string {
  const fileName = path.split('/').at(-1)?.toLowerCase() ?? ''
  const extensionIndex = fileName.lastIndexOf('.')
  return extensionIndex >= 0 ? fileName.slice(extensionIndex) : ''
}
