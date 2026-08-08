import { useEffect, useRef } from "react";
import Editor, { DiffEditor, loader, type DiffOnMount, type OnMount } from "@monaco-editor/react";
import { monaco } from "./monaco/setup";

loader.config({ monaco });

interface CodeEditorProps {
  path: string;
  modelPath?: string;
  language: string;
  value: string;
  readOnly: boolean;
  lineNumberOffset?: number;
  onChange?: (value: string) => void;
  onSave?: () => void | Promise<void>;
}

export function CodeEditor({
  path,
  modelPath,
  language,
  value,
  readOnly,
  lineNumberOffset = 1,
  onChange,
  onSave,
}: CodeEditorProps) {
  const saveRef = useRef(onSave);
  saveRef.current = onSave;
  const editorRef = useRef<Parameters<OnMount>[0] | null>(null);

  useEffect(() => {
    editorRef.current?.updateOptions({ readOnly });
  }, [readOnly]);

  const mount: OnMount = (editor, api) => {
    editorRef.current = editor;
    editor.getModel()?.setEOL(
      value.includes("\r\n")
        ? api.editor.EndOfLineSequence.CRLF
        : api.editor.EndOfLineSequence.LF,
    );
    if (saveRef.current) {
      editor.addCommand(api.KeyMod.CtrlCmd | api.KeyCode.KeyS, () => {
        void saveRef.current?.();
      });
    }
  };

  const theme = document.documentElement.dataset.theme === "dark" ? "vs-dark" : "vs";

  return (
    <div className="code-editor" data-language={language}>
      <Editor
        path={modelPath ?? path}
        language={language || "plaintext"}
        value={value}
        theme={theme}
        loading={<div className="code-editor-loading">正在载入编辑器...</div>}
        onMount={mount}
        onChange={onChange ? (next) => onChange(next ?? "") : undefined}
        options={{
          ariaLabel: `${readOnly ? "查看" : "编辑"} ${path}`,
          automaticLayout: true,
          contextmenu: true,
          cursorBlinking: "smooth",
          domReadOnly: readOnly,
          folding: true,
          fontFamily: "var(--font-family-mono)",
          fontLigatures: false,
          fontSize: 12,
          glyphMargin: false,
          lineDecorationsWidth: 8,
          lineHeight: 19,
          lineNumbers: lineNumberOffset > 1
            ? (lineNumber) => String(lineNumberOffset + lineNumber - 1)
            : "on",
          lineNumbersMinChars: 3,
          minimap: { enabled: false },
          padding: { top: 7, bottom: 7 },
          readOnly,
          renderLineHighlight: "all",
          roundedSelection: false,
          scrollBeyondLastLine: false,
          smoothScrolling: true,
          tabSize: 2,
          wordWrap: "off",
        }}
      />
    </div>
  );
}

interface CodeDiffEditorProps {
  path: string;
  originalModelPath: string;
  modifiedModelPath: string;
  language: string;
  originalValue: string;
  modifiedValue: string;
}

export function CodeDiffEditor({
  path,
  originalModelPath,
  modifiedModelPath,
  language,
  originalValue,
  modifiedValue,
}: CodeDiffEditorProps) {
  const mount: DiffOnMount = (editor, api) => {
    editor.getOriginalEditor().getModel()?.setEOL(
      originalValue.includes("\r\n")
        ? api.editor.EndOfLineSequence.CRLF
        : api.editor.EndOfLineSequence.LF,
    );
    editor.getModifiedEditor().getModel()?.setEOL(
      modifiedValue.includes("\r\n")
        ? api.editor.EndOfLineSequence.CRLF
        : api.editor.EndOfLineSequence.LF,
    );
  };
  const theme = document.documentElement.dataset.theme === "dark" ? "vs-dark" : "vs";

  return (
    <div className="code-diff-editor" data-language={language}>
      <DiffEditor
        originalModelPath={originalModelPath}
        modifiedModelPath={modifiedModelPath}
        originalLanguage={language || "plaintext"}
        modifiedLanguage={language || "plaintext"}
        original={originalValue}
        modified={modifiedValue}
        theme={theme}
        loading={<div className="code-editor-loading">正在载入编辑器...</div>}
        onMount={mount}
        options={{
          automaticLayout: true,
          contextmenu: true,
          diffAlgorithm: "advanced",
          enableSplitViewResizing: false,
          fontFamily: "var(--font-family-mono)",
          fontLigatures: false,
          fontSize: 12,
          glyphMargin: false,
          lineDecorationsWidth: 8,
          lineHeight: 19,
          lineNumbers: "on",
          lineNumbersMinChars: 3,
          minimap: { enabled: false },
          modifiedAriaLabel: `修改后 ${path}`,
          originalAriaLabel: `修改前 ${path}`,
          originalEditable: false,
          padding: { top: 7, bottom: 7 },
          readOnly: true,
          renderIndicators: true,
          renderLineHighlight: "all",
          renderMarginRevertIcon: false,
          renderSideBySide: false,
          roundedSelection: false,
          scrollBeyondLastLine: false,
          smoothScrolling: true,
          wordWrap: "off",
        }}
      />
    </div>
  );
}
