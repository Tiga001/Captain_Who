// Renderer UI.
import { useEffect } from "react";
import { useFrontendConfig } from "../../config/FrontendConfigProvider";
import type { BrowserPageMetadata } from "./browserClient";
import "./BrowserPanel.css";

interface BrowserPanelProps {
  isActive: boolean;
  onPageMetadataChange?: (metadata: BrowserPageMetadata) => void;
  pageId: string;
}

export function BrowserPanel({ isActive, onPageMetadataChange, pageId }: BrowserPanelProps) {
  const { t } = useFrontendConfig();

  useEffect(() => {
    onPageMetadataChange?.({ title: t("browser.title"), iconUrl: null });
  }, [onPageMetadataChange, t]);

  return (
    <section className="browser-panel browser-panel--placeholder" data-active={isActive || undefined}>
      <div className="browser-panel__placeholder">
        <h2>{t("browser.emptyTitle")}</h2>
        <p>{t("browser.emptyDescription")}</p>
        <code>{pageId}</code>
      </div>
    </section>
  );
}
