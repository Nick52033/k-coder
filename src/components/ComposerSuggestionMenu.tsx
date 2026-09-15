import { AtSign, Code2, FileText, Image as ImageIcon, Settings2, Sparkles } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { FileEntry, SkillDiagnostic } from "../types/runtime";
import { cn } from "../lib/cn";

export type ComposerSuggestion =
  | { kind: "file"; entry: FileEntry }
  | { kind: "skill"; skill: SkillDiagnostic };

interface ComposerSuggestionMenuProps {
  kind: "file" | "skill";
  suggestions: ComposerSuggestion[];
  activeIndex: number;
  loading: boolean;
  error: string;
  onSelect: (suggestion: ComposerSuggestion) => void;
}

type FileSuggestionCategory = "code" | "document" | "config" | "asset" | "other";

const FILE_CATEGORY_META: ReadonlyArray<{
  id: FileSuggestionCategory;
  label: string;
  Icon: LucideIcon;
}> = [
  { id: "code", label: "代码", Icon: Code2 },
  { id: "document", label: "文档与数据", Icon: FileText },
  { id: "config", label: "配置", Icon: Settings2 },
  { id: "asset", label: "样式与资源", Icon: ImageIcon },
  { id: "other", label: "其他文件", Icon: FileText },
];

const FILE_CATEGORY_EXTENSIONS: Record<Exclude<FileSuggestionCategory, "other">, ReadonlySet<string>> = {
  code: new Set([
    "c", "cc", "cpp", "cxx", "h", "hh", "hpp", "cs", "go", "java", "js", "jsx", "mjs", "cjs",
    "kt", "kts", "php", "py", "rb", "rs", "swift", "ts", "tsx", "vue", "svelte", "astro", "sql", "sh", "bash", "ps1",
  ]),
  document: new Set(["csv", "doc", "docx", "md", "mdx", "pdf", "ppt", "pptx", "rtf", "txt", "tsv", "xls", "xlsx", "xlsm"]),
  config: new Set(["conf", "config", "env", "ini", "json", "jsonc", "properties", "toml", "xml", "yaml", "yml"]),
  asset: new Set(["bmp", "css", "gif", "htm", "html", "ico", "jpeg", "jpg", "less", "otf", "png", "sass", "scss", "svg", "ttf", "webp", "woff", "woff2"]),
};

function getFileSuggestionCategory(path: string): FileSuggestionCategory {
  const fileName = path.split("/").pop()?.toLowerCase() ?? "";
  const extensionStart = fileName.lastIndexOf(".");
  const extension = extensionStart > 0 ? fileName.slice(extensionStart + 1) : "";
  for (const category of ["code", "document", "config", "asset"] as const) {
    if (FILE_CATEGORY_EXTENSIONS[category].has(extension)) return category;
  }
  return "other";
}

export function ComposerSuggestionMenu({
  kind,
  suggestions,
  activeIndex,
  loading,
  error,
  onSelect,
}: ComposerSuggestionMenuProps) {
  const groupedFiles = kind === "file"
    ? FILE_CATEGORY_META.map((category) => ({
      ...category,
      entries: suggestions
        .map((suggestion, index) => ({ suggestion, index }))
        .filter(({ suggestion }) => suggestion.kind === "file" && getFileSuggestionCategory(suggestion.entry.path) === category.id),
    })).filter((category) => category.entries.length > 0)
    : [];

  const renderSuggestion = (suggestion: ComposerSuggestion, index: number) => (
    <SuggestionOption
      key={suggestion.kind === "file" ? suggestion.entry.path : suggestion.skill.name}
      suggestion={suggestion}
      index={index}
      activeIndex={activeIndex}
      onSelect={onSelect}
    />
  );

  return (
    <div className="composer-suggestions" role="listbox" aria-label={kind === "file" ? "文件引用" : "Skills"}>
      <div className="composer-suggestions-header">
        {kind === "file" ? <AtSign size={14} /> : <Sparkles size={14} />}
        <span>{kind === "file" ? "文件" : "Skills"}</span>
        {loading && <small>加载中</small>}
        {!loading && !error && suggestions.length > 0 && <small>{suggestions.length} 项</small>}
      </div>
      {loading && <div className="composer-suggestions-empty">正在查找...</div>}
      {!loading && error && <div className="composer-suggestions-empty composer-suggestions-empty--error">{error}</div>}
      {!loading && !error && suggestions.length === 0 && (
        <div className="composer-suggestions-empty">{kind === "file" ? "没有匹配的文件" : "没有已启用的 Skill"}</div>
      )}
      {!loading && !error && kind === "file" && groupedFiles.map((category) => (
        <div className="composer-suggestion-group" role="group" aria-label={`${category.label}，${category.entries.length} 项`} key={category.id}>
          <div className="composer-suggestion-group-heading composer-suggestion-group-title">
            <category.Icon size={13} aria-hidden="true" />
            <span>{category.label}</span>
            <small>{category.entries.length}</small>
          </div>
          {category.entries.map(({ suggestion, index }) => renderSuggestion(suggestion, index))}
        </div>
      ))}
      {!loading && !error && kind === "skill" && suggestions.map(renderSuggestion)}
    </div>
  );
}

function SuggestionOption({
  suggestion,
  index,
  activeIndex,
  onSelect,
}: {
  suggestion: ComposerSuggestion;
  index: number;
  activeIndex: number;
  onSelect: (suggestion: ComposerSuggestion) => void;
}) {
  const disabled = suggestion.kind === "skill" && !suggestion.skill.enabled;
  const label = suggestion.kind === "file" ? suggestion.entry.path : `/${suggestion.skill.name}`;
  const detail = suggestion.kind === "file"
    ? (suggestion.entry.size == null ? "文件" : formatBytes(suggestion.entry.size))
    : `${suggestion.skill.description || "无描述"} · ${suggestion.skill.scope}`;

  return (
    <button
      type="button"
      role="option"
      aria-selected={index === activeIndex}
      aria-disabled={disabled || undefined}
      disabled={disabled}
      className={cn("composer-suggestion", index === activeIndex && "composer-suggestion--active", disabled && "composer-suggestion--disabled")}
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => onSelect(suggestion)}
    >
      {suggestion.kind === "file" ? <FileText size={15} /> : <Sparkles size={15} />}
      <span className="composer-suggestion-main">
        <strong>{label}</strong>
        <small>{detail}</small>
      </span>
      {disabled && <em>未启用</em>}
    </button>
  );
}

function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${Math.round(size / 1024)} KiB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MiB`;
}
