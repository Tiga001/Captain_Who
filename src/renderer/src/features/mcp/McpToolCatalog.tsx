import { AlertTriangle, LoaderCircle, RefreshCw, Wrench } from 'lucide-react'
import type { McpServerListItem } from '@mycopilot/protocol'
import { useFrontendConfig } from '../../config/FrontendConfigProvider'
import { McpCatalogCompletenessBadge } from './McpServerStatusBadge'
import { shortMcpFingerprint, toSafeMcpDisplayText } from './mcpSafeDisplay'
import type { McpToolCatalogState } from './useMcpManagement'

interface McpToolCatalogProps {
  catalog: McpToolCatalogState
  onLoad: () => void
  onLoadMore: () => void
  onRefresh: () => void
  server: McpServerListItem
}

export function McpToolCatalog({
  catalog,
  onLoad,
  onLoadMore,
  onRefresh,
  server
}: McpToolCatalogProps) {
  const { t } = useFrontendConfig()

  return (
    <section className="mcp-tool-catalog">
      <header>
        <div>
          <h3>{t('mcp.catalog.title')}</h3>
          <p>{t('mcp.catalog.safeProjectionHelp')}</p>
        </div>
        <div className="mcp-catalog-actions">
          {catalog.catalogCompleteness && (
            <McpCatalogCompletenessBadge completeness={catalog.catalogCompleteness} />
          )}
          <button
            aria-label={t('mcp.catalog.refresh')}
            className="mcp-icon-button"
            disabled={catalog.isRefreshing}
            onClick={onRefresh}
            type="button"
          >
            <RefreshCw aria-hidden="true" />
          </button>
        </div>
      </header>

      {catalog.status === 'idle' && (
        <button className="mcp-secondary-button" onClick={onLoad} type="button">
          {t('mcp.catalog.load')}
        </button>
      )}

      {catalog.status === 'loading' && (
        <div className="mcp-catalog-state" role="status">
          <LoaderCircle aria-hidden="true" className="mcp-spinner" />
          {t('mcp.catalog.loading')}
        </div>
      )}

      {catalog.errorMessage !== null && (
        <div className="mcp-safe-error" role="alert">
          <AlertTriangle aria-hidden="true" />
          <p>
            {catalog.errorMessage
              ? toSafeMcpDisplayText(catalog.errorMessage)
              : t('mcp.error.unknown')}
          </p>
          <button className="mcp-secondary-button" onClick={onLoad} type="button">
            {t('mcp.actions.retry')}
          </button>
        </div>
      )}

      {catalog.status === 'ready' && catalog.tools.length === 0 && (
        <div className="mcp-catalog-empty" role="status">
          <Wrench aria-hidden="true" />
          <strong>
            {catalog.catalogCompleteness === 'complete'
              ? t('mcp.catalog.empty')
              : t('mcp.catalog.unavailable')}
          </strong>
          <p>
            {catalog.catalogCompleteness === 'complete'
              ? t('mcp.catalog.emptyDescription')
              : t('mcp.catalog.incompleteDescription')}
          </p>
        </div>
      )}

      {catalog.tools.length > 0 && (
        <div className="mcp-tool-list">
          {catalog.tools.map((tool) => (
            <article
              className="mcp-tool-row"
              key={`${tool.serverId}:${tool.rawName}:${tool.modelName}`}
            >
              <div className="mcp-tool-row__heading">
                <h4>{toSafeMcpDisplayText(tool.rawName, 1024)}</h4>
                <span>{toSafeMcpDisplayText(tool.modelName, 64)}</span>
                <span
                  className="mcp-routability-badge"
                  data-state={tool.routable && !tool.disabled ? 'routable' : 'disabled'}
                >
                  {tool.routable && !tool.disabled
                    ? t('mcp.catalog.routable')
                    : t('mcp.catalog.disabled')}
                </span>
              </div>
              <p>{toSafeMcpDisplayText(tool.description, 1024)}</p>
              {tool.descriptionTruncated && <small>{t('mcp.catalog.descriptionTruncated')}</small>}
              <dl>
                <div>
                  <dt>{t('mcp.catalog.schemaFingerprint')}</dt>
                  <dd>{shortMcpFingerprint(tool.schemaDigestPrefix)}</dd>
                </div>
                <div>
                  <dt>{t('mcp.detail.catalogGeneration')}</dt>
                  <dd>{tool.catalogGeneration}</dd>
                </div>
              </dl>
              {tool.diagnosticCodes.length > 0 && (
                <div className="mcp-tool-diagnostics" role="status">
                  <AlertTriangle aria-hidden="true" />
                  <ul>
                    {tool.diagnosticCodes.map((code) => (
                      <li key={code}>{toSafeMcpDisplayText(code, 128)}</li>
                    ))}
                  </ul>
                </div>
              )}
            </article>
          ))}
        </div>
      )}

      {catalog.nextCursor && (
        <button
          className="mcp-secondary-button mcp-load-more"
          disabled={catalog.isRefreshing}
          onClick={onLoadMore}
          type="button"
        >
          {catalog.isRefreshing ? t('mcp.catalog.loadingMore') : t('mcp.catalog.loadMore')}
        </button>
      )}

      <p className="mcp-catalog-provenance">
        {t('mcp.catalog.server')}: {toSafeMcpDisplayText(server.displayName, 256)}
      </p>
    </section>
  )
}
