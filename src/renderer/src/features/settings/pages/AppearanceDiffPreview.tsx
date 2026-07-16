import type { ReactNode } from 'react'

interface AppearanceDiffPreviewProps {
  label: string
  themeId: string
}

type PreviewLineKind = 'addition' | 'context' | 'deletion'

interface PreviewLine {
  code: ReactNode
  kind: PreviewLineKind
  lineNumber: number
}

const contextStart = (
  <>
    <span className="appearance-diff-preview__syntax-keyword">const</span>{' '}
    <span className="appearance-diff-preview__syntax-variable">themePreview</span>
    <span className="appearance-diff-preview__syntax-operator">: </span>
    <span className="appearance-diff-preview__syntax-type">ThemeConfig</span>
    <span className="appearance-diff-preview__syntax-operator"> = {'{'}</span>
  </>
)

const contextEnd = <span className="appearance-diff-preview__syntax-operator">{'};'}</span>

function propertyLine(name: string, value: ReactNode): ReactNode {
  return (
    <>
      {'  '}
      <span className="appearance-diff-preview__syntax-attribute">{name}</span>
      <span className="appearance-diff-preview__syntax-operator">: </span>
      {value}
      <span className="appearance-diff-preview__syntax-operator">,</span>
    </>
  )
}

const deletedLines: PreviewLine[] = [
  { code: contextStart, kind: 'context', lineNumber: 1 },
  {
    code: propertyLine(
      'surface',
      <span className="appearance-diff-preview__syntax-string">&quot;sidebar&quot;</span>
    ),
    kind: 'deletion',
    lineNumber: 2
  },
  {
    code: propertyLine(
      'accent',
      <span className="appearance-diff-preview__syntax-string">&quot;#2563eb&quot;</span>
    ),
    kind: 'deletion',
    lineNumber: 3
  },
  {
    code: propertyLine(
      'contrast',
      <span className="appearance-diff-preview__syntax-number">42</span>
    ),
    kind: 'deletion',
    lineNumber: 4
  },
  { code: contextEnd, kind: 'context', lineNumber: 5 }
]

const addedLines: PreviewLine[] = [
  { code: contextStart, kind: 'context', lineNumber: 1 },
  {
    code: propertyLine(
      'surface',
      <span className="appearance-diff-preview__syntax-string">&quot;sidebar-elevated&quot;</span>
    ),
    kind: 'addition',
    lineNumber: 2
  },
  {
    code: propertyLine(
      'accent',
      <span className="appearance-diff-preview__syntax-string">&quot;#0ea5e9&quot;</span>
    ),
    kind: 'addition',
    lineNumber: 3
  },
  {
    code: propertyLine(
      'contrast',
      <span className="appearance-diff-preview__syntax-number">68</span>
    ),
    kind: 'addition',
    lineNumber: 4
  },
  { code: contextEnd, kind: 'context', lineNumber: 5 }
]

function PreviewPane({ lines }: { lines: PreviewLine[] }): ReactNode {
  return (
    <div className="appearance-diff-preview__pane">
      {lines.map((line) => (
        <div
          className="appearance-diff-preview__line"
          data-kind={line.kind}
          key={`${line.kind}-${line.lineNumber}`}
        >
          <span className="appearance-diff-preview__line-number">{line.lineNumber}</span>
          <code>{line.code}</code>
        </div>
      ))}
    </div>
  )
}

export function AppearanceDiffPreview({ label, themeId }: AppearanceDiffPreviewProps): ReactNode {
  return (
    <div aria-label={label} className="appearance-diff-preview" data-theme-id={themeId} role="img">
      <div className="appearance-diff-preview__canvas">
        <PreviewPane lines={deletedLines} />
        <PreviewPane lines={addedLines} />
      </div>
    </div>
  )
}
