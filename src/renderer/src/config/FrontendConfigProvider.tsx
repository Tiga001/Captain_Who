// Renderer UI.
import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import {
  FRONTEND_CONFIG_STORAGE_KEY,
  frontendConfig,
  getFrontendCssVariables,
} from "./frontendConfig";
import { frontendThemes } from "./frontendTheme";
import { languageOptions, translations } from "./frontendTranslations";
import type { FrontendConfig } from "./frontendConfig";
import type { FrontendThemeName, ThemePreference } from "./frontendTheme";
import type { AppLanguage, TranslationKey } from "./frontendTranslations";

interface StoredFrontendConfig {
  language?: AppLanguage;
  themePreference?: ThemePreference;
}

interface FrontendConfigContextValue {
  config: FrontendConfig;
  language: AppLanguage;
  languageOptions: typeof languageOptions;
  resolvedTheme: FrontendThemeName;
  setLanguage: (language: AppLanguage) => void;
  setThemePreference: (themePreference: ThemePreference) => void;
  t: (key: TranslationKey) => string;
  themePreference: ThemePreference;
}

const FrontendConfigContext = createContext<FrontendConfigContextValue | null>(null);

function isAppLanguage(value: string | null | undefined): value is AppLanguage {
  return value === "zh-CN" || value === "en-US";
}

function isThemePreference(value: string | null | undefined): value is ThemePreference {
  return value === "system" || value === "light" || value === "dark";
}

function getSystemTheme(): FrontendThemeName {
  if (!window.matchMedia) return "light";
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function readStoredConfig(): Required<StoredFrontendConfig> {
  try {
    const rawValue = window.localStorage.getItem(FRONTEND_CONFIG_STORAGE_KEY);
    if (!rawValue) {
      return {
        language: frontendConfig.language,
        themePreference: frontendConfig.themePreference,
      };
    }

    const parsed = JSON.parse(rawValue) as StoredFrontendConfig;
    return {
      language: isAppLanguage(parsed.language) ? parsed.language : frontendConfig.language,
      themePreference: isThemePreference(parsed.themePreference)
        ? parsed.themePreference
        : frontendConfig.themePreference,
    };
  } catch {
    return {
      language: frontendConfig.language,
      themePreference: frontendConfig.themePreference,
    };
  }
}

export function FrontendConfigProvider({ children }: { children: ReactNode }) {
  const [initialConfig] = useState(() => readStoredConfig());
  const [language, setLanguage] = useState<AppLanguage>(initialConfig.language);
  const [systemTheme, setSystemTheme] = useState<FrontendThemeName>(() => getSystemTheme());
  const [themePreference, setThemePreference] = useState<ThemePreference>(initialConfig.themePreference);
  const resolvedTheme = themePreference === "system" ? systemTheme : themePreference;

  useLayoutEffect(() => {
    const variables = getFrontendCssVariables(frontendConfig, frontendThemes[resolvedTheme]);
    Object.entries(variables).forEach(([name, value]) => {
      document.documentElement.style.setProperty(name, value);
    });

    document.documentElement.dataset.theme = resolvedTheme;
    document.documentElement.dataset.themePreference = themePreference;
    document.documentElement.style.colorScheme = resolvedTheme;
  }, [resolvedTheme, themePreference]);

  useEffect(() => {
    if (!window.matchMedia) return;

    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const handleThemeChange = () => {
      setSystemTheme(mediaQuery.matches ? "dark" : "light");
    };

    handleThemeChange();
    mediaQuery.addEventListener("change", handleThemeChange);
    return () => mediaQuery.removeEventListener("change", handleThemeChange);
  }, []);

  useEffect(() => {
    document.documentElement.lang = language;
    window.localStorage.setItem(FRONTEND_CONFIG_STORAGE_KEY, JSON.stringify({ language, themePreference }));
  }, [language, themePreference]);

  const value = useMemo<FrontendConfigContextValue>(
    () => ({
      config: frontendConfig,
      language,
      languageOptions,
      resolvedTheme,
      setLanguage,
      setThemePreference,
      t: (key) => translations[language][key] ?? translations[frontendConfig.language][key],
      themePreference,
    }),
    [language, resolvedTheme, themePreference],
  );

  return <FrontendConfigContext.Provider value={value}>{children}</FrontendConfigContext.Provider>;
}

export function useFrontendConfig() {
  const context = useContext(FrontendConfigContext);

  if (!context) {
    throw new Error("useFrontendConfig must be used within FrontendConfigProvider");
  }

  return context;
}
