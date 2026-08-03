import { useEffect, useRef } from "react";
import Editor, { loader, type OnMount } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import CssWorker from "monaco-editor/language/css/css.worker?worker";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import HtmlWorker from "monaco-editor/language/html/html.worker?worker";
import JsonWorker from "monaco-editor/language/json/json.worker?worker";
import TypeScriptWorker from "monaco-editor/language/typescript/ts.worker?worker";

loader.config({ monaco });

const monacoGlobal = globalThis as typeof globalThis & {
  MonacoEnvironment?: { getWorker: (moduleId: string, label: string) => Worker };
};

monacoGlobal.MonacoEnvironment = {
  getWorker(_moduleId, label) {
    if (label === "json") return new JsonWorker();
    if (label === "css" || label === "scss" || label === "less") return new CssWorker();
    if (label === "html" || label === "handlebars" || label === "razor") return new HtmlWorker();
    if (label === "typescript" || label === "javascript") return new TypeScriptWorker();
    return new EditorWorker();
  },
};

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
