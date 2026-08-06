import { AtSign, FileText, Sparkles } from "lucide-react";
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

export function ComposerSuggestionMenu({
  kind,
  suggestions,
  activeIndex,
  loading,
  error,
  onSelect,
}: ComposerSuggestionMenuProps) {
  return (
    <div className="composer-suggestions" role="listbox" aria-label={kind === "file" ? "文件引用" : "Skills"}>
      <div className="composer-suggestions-header">
        {kind === "file" ? <AtSign size={14} /> : <Sparkles size={14} />}
        <span>{kind === "file" ? "文件" : "Skills"}</span>
        {loading && <small>加载中</small>}
      </div>
      {loading && <div className="composer-suggestions-empty">正在查找...</div>}
      {!loading && error && <div className="composer-suggestions-empty composer-suggestions-empty--error">{error}</div>}
      {!loading && !error && suggestions.length === 0 && (
        <div className="composer-suggestions-empty">{kind === "file" ? "没有匹配的文件" : "没有已启用的 Skill"}</div>
      )}
      {!loading && !error && suggestions.map((suggestion, index) => {
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
            key={suggestion.kind === "file" ? suggestion.entry.path : suggestion.skill.name}
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
      })}
    </div>
  );
}

function formatBytes(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${Math.round(size / 1024)} KiB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MiB`;
}
