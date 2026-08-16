import { useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, Folder, FolderPlus, FolderX, Loader2, Search } from "lucide-react";
import type { ProjectRecord } from "../types/runtime";
import { cn } from "../lib/cn";
import { toUserFacingPath, workspacePathsEqual } from "../lib/path";

interface ProjectSelectorProps {
  projects: ProjectRecord[];
  activeProject: ProjectRecord | null;
  standalone: boolean;
  disabled?: boolean;
  switching?: boolean;
  onSelect: (project: ProjectRecord) => Promise<void> | void;
  onCreateProject: () => Promise<void> | void;
  onSelectStandalone: () => Promise<void> | void;
}

export function ProjectSelector({
  projects,
  activeProject,
  standalone,
  disabled = false,
  switching = false,
  onSelect,
  onCreateProject,
  onSelectStandalone,
}: ProjectSelectorProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [selecting, setSelecting] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const unavailable = disabled || switching || selecting;
  const filteredProjects = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    if (!normalizedQuery) return projects;
    return projects.filter((project) =>
      `${project.name}\n${toUserFacingPath(project.path)}`.toLocaleLowerCase().includes(normalizedQuery),
    );
  }, [projects, query]);

  useEffect(() => {
    if (!open) return undefined;
    const handlePointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const handleKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", handlePointerDown);
    window.addEventListener("keydown", handleKeyDown);
    requestAnimationFrame(() => searchRef.current?.focus());
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, [open]);

  const close = () => {
    setOpen(false);
    setQuery("");
  };

  return (
    <div className="project-selector" ref={rootRef}>
      <button
        type="button"
        className="project-selector-trigger"
        aria-label={standalone ? "当前不在项目中" : activeProject ? `当前项目 ${activeProject.name}` : "选择项目"}
        aria-expanded={open}
        aria-haspopup="dialog"
        title={standalone ? "当前会话不使用项目工作区" : activeProject ? toUserFacingPath(activeProject.path) : "选择当前对话的项目"}
        disabled={unavailable}
        onClick={() => setOpen((value) => !value)}
      >
        {switching || selecting ? (
          <Loader2 className="spin" size={15} />
        ) : standalone ? (
          <FolderX size={15} />
        ) : (
          <Folder size={15} />
        )}
        <span>{standalone ? "不在项目中" : activeProject?.name ?? "选择项目"}</span>
        <ChevronDown className="project-selector-chevron" size={13} />
      </button>

      {open ? (
        <div className="project-selector-menu" role="dialog" aria-label="选择项目">
          <label className="project-selector-search">
            <Search size={14} aria-hidden="true" />
            <input
              ref={searchRef}
              value={query}
              onChange={(event) => setQuery(event.currentTarget.value)}
              aria-label="搜索项目"
              placeholder="搜索项目"
            />
          </label>
          <div className="project-selector-options" role="listbox" aria-label="最近项目">
            {filteredProjects.map((project) => {
              const displayPath = toUserFacingPath(project.path);
              const active = activeProject ? workspacePathsEqual(project.path, activeProject.path) : false;
              return (
                <button
                  key={project.id || project.path}
                  type="button"
                  className={cn("project-selector-option", active && "project-selector-option--active")}
                  role="option"
                  aria-selected={active}
                  title={displayPath}
                  disabled={unavailable}
                  onClick={async () => {
                    if (active) {
                      close();
                      return;
                    }
                    setSelecting(true);
                    try {
                      await onSelect(project);
                      close();
                    } finally {
                      setSelecting(false);
                    }
                  }}
                >
                  <Folder size={15} aria-hidden="true" />
                  <span>
                    <strong>{project.name}</strong>
                    <small>{displayPath}</small>
                  </span>
                  {active ? <Check size={15} aria-hidden="true" /> : null}
                </button>
              );
            })}
            {!filteredProjects.length ? (
              <div className="project-selector-empty">没有匹配的项目</div>
            ) : null}
          </div>
          <div className="project-selector-footer">
            <button
              type="button"
              disabled={unavailable}
              onClick={async () => {
                close();
                setSelecting(true);
                try {
                  await onCreateProject();
                } finally {
                  setSelecting(false);
                }
              }}
            >
              <FolderPlus size={15} aria-hidden="true" />
              <span>新建项目</span>
            </button>
            <button
              type="button"
              className={cn(standalone && "project-selector-footer-option--active")}
              aria-pressed={standalone}
              disabled={unavailable}
              onClick={async () => {
                if (standalone) {
                  close();
                  return;
                }
                close();
                setSelecting(true);
                try {
                  await onSelectStandalone();
                } finally {
                  setSelecting(false);
                }
              }}
            >
              <FolderX size={15} aria-hidden="true" />
              <span>不在项目中</span>
              {standalone ? <Check className="project-selector-footer-check" size={15} aria-hidden="true" /> : null}
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
