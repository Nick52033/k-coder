export type ThemeId =
  | "light"
  | "dark"
  | "arctic"
  | "crt-green"
  | "ember"
  | "miami"
  | "synthwave"
  | "terminal"
  | "vapor"
  | "system";

export type ResolvedThemeId = Exclude<ThemeId, "system">;

export interface ThemeOption {
  id: ThemeId;
  label: string;
  description: string;
  icon: "sun" | "moon" | "cloud" | "monitor" | "flame" | "sparkles" | "terminal" | "waves";
  swatch: [string, string];
}

export const THEME_STORAGE_KEY = "kcoder_theme";

export const THEME_OPTIONS: ThemeOption[] = [
  { id: "light", label: "浅色", description: "清晰明亮", icon: "sun", swatch: ["#f8fafc", "#2f6fe4"] },
  { id: "dark", label: "深色", description: "柔和暗色", icon: "moon", swatch: ["#1b2030", "#4f8aff"] },
  { id: "arctic", label: "Arctic", description: "冰川蓝白", icon: "cloud", swatch: ["#f2f8fb", "#1687a7"] },
  { id: "crt-green", label: "CRT Green", description: "荧光绿屏", icon: "monitor", swatch: ["#08110c", "#78f6a5"] },
  { id: "ember", label: "Ember", description: "炭火橙红", icon: "flame", swatch: ["#201719", "#f97316"] },
  { id: "miami", label: "Miami", description: "海盐珊瑚", icon: "sun", swatch: ["#fff7f4", "#e66064"] },
  { id: "synthwave", label: "Synthwave", description: "霓虹夜色", icon: "moon", swatch: ["#171326", "#f472b6"] },
  { id: "terminal", label: "Terminal", description: "琥珀终端", icon: "terminal", swatch: ["#0f1110", "#f5b94c"] },
  { id: "vapor", label: "Vapor", description: "雾紫柔光", icon: "waves", swatch: ["#faf8ff", "#8b73d6"] },
  { id: "system", label: "跟随系统", description: "使用系统偏好", icon: "monitor", swatch: ["#f2f5fa", "#4f8aff"] },
];

const THEME_IDS = new Set<ThemeId>(THEME_OPTIONS.map((option) => option.id));

export function isThemeId(value: unknown): value is ThemeId {
  return typeof value === "string" && THEME_IDS.has(value as ThemeId);
}

export function parseThemePreference(value: unknown): ThemeId {
  return isThemeId(value) ? value : "light";
}

export function resolveTheme(preference: ThemeId, systemPrefersDark: boolean): ResolvedThemeId {
  if (preference !== "system") return preference;
  return systemPrefersDark ? "dark" : "light";
}

export function themeLabel(theme: ThemeId): string {
  return THEME_OPTIONS.find((option) => option.id === theme)?.label ?? "浅色";
}
