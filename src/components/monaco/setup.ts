// 最小化 Monaco 装配：editor.api 核心 + 当前工作台实际用到的编辑器功能 + 白名单语言高亮。
// 不注册 css/html/json/typescript 语言服务；全部语言共用唯一 editor.worker，
// TypeScript worker 不进入任何加载路径（非 TS 预览不可能请求它，TS 文件也不再注册语言服务）。
// 深路径经 vite alias 与 tsconfig paths 直接映射到 monaco-editor/esm/vs。
import * as monaco from "monaco-editor/esm/vs/editor/editor.api.js";

// 编辑器核心部件与命令
import "monaco-editor/esm/vs/editor/browser/widget/codeEditor/codeEditorWidget.js";
import "monaco-editor/esm/vs/editor/browser/widget/diffEditor/diffEditor.contribution.js";
import "monaco-editor/esm/vs/editor/browser/coreCommands.js";
import "monaco-editor/esm/vs/editor/common/standaloneStrings.js";

// 工作台交互保留的功能
import "monaco-editor/esm/vs/editor/contrib/bracketMatching/browser/bracketMatching.js";
import "monaco-editor/esm/vs/editor/contrib/caretOperations/browser/caretOperations.js";
import "monaco-editor/esm/vs/editor/contrib/clipboard/browser/clipboard.js";
import "monaco-editor/esm/vs/editor/contrib/comment/browser/comment.js";
import "monaco-editor/esm/vs/editor/contrib/contextmenu/browser/contextmenu.js";
import "monaco-editor/esm/vs/editor/contrib/cursorUndo/browser/cursorUndo.js";
import "monaco-editor/esm/vs/editor/contrib/dnd/browser/dnd.js";
import "monaco-editor/esm/vs/editor/contrib/find/browser/findController.js";
import "monaco-editor/esm/vs/editor/contrib/folding/browser/folding.js";
import "monaco-editor/esm/vs/editor/contrib/hover/browser/hoverContribution.js";
import "monaco-editor/esm/vs/editor/contrib/indentation/browser/indentation.js";
import "monaco-editor/esm/vs/editor/contrib/lineSelection/browser/lineSelection.js";
import "monaco-editor/esm/vs/editor/contrib/linesOperations/browser/linesOperations.js";
import "monaco-editor/esm/vs/editor/contrib/links/browser/links.js";
import "monaco-editor/esm/vs/editor/contrib/multicursor/browser/multicursor.js";
import "monaco-editor/esm/vs/editor/contrib/readOnlyMessage/browser/contribution.js";
import "monaco-editor/esm/vs/editor/contrib/smartSelect/browser/smartSelect.js";
import "monaco-editor/esm/vs/editor/contrib/tokenization/browser/tokenization.js";
import "monaco-editor/esm/vs/editor/contrib/unusualLineTerminators/browser/unusualLineTerminators.js";
import "monaco-editor/esm/vs/editor/contrib/wordHighlighter/browser/wordHighlighter.js";
import "monaco-editor/esm/vs/editor/contrib/wordOperations/browser/wordOperations.js";
import "monaco-editor/esm/vs/editor/contrib/wordPartOperations/browser/wordPartOperations.js";
import "monaco-editor/esm/vs/editor/standalone/browser/quickAccess/standaloneGotoLineQuickAccess.js";

// 图标字体
import "monaco-editor/esm/vs/base/browser/ui/codicons/codicon/codicon.css";
import "monaco-editor/esm/vs/base/browser/ui/codicons/codicon/codicon-modifiers.css";

// 工作台语言白名单（register.js 仅注册，语法定义按需加载）
import "monaco-editor/esm/vs/languages/definitions/bat/register.js";
import "monaco-editor/esm/vs/languages/definitions/cpp/register.js";
import "monaco-editor/esm/vs/languages/definitions/csharp/register.js";
import "monaco-editor/esm/vs/languages/definitions/css/register.js";
import "monaco-editor/esm/vs/languages/definitions/dockerfile/register.js";
import "monaco-editor/esm/vs/languages/definitions/go/register.js";
import "monaco-editor/esm/vs/languages/definitions/html/register.js";
import "monaco-editor/esm/vs/languages/definitions/ini/register.js";
import "monaco-editor/esm/vs/languages/definitions/java/register.js";
import "monaco-editor/esm/vs/languages/definitions/javascript/register.js";
import "monaco-editor/esm/vs/languages/definitions/less/register.js";
import "monaco-editor/esm/vs/languages/definitions/markdown/register.js";
import "monaco-editor/esm/vs/languages/definitions/mdx/register.js";
import "monaco-editor/esm/vs/languages/definitions/php/register.js";
import "monaco-editor/esm/vs/languages/definitions/powershell/register.js";
import "monaco-editor/esm/vs/languages/definitions/python/register.js";
import "monaco-editor/esm/vs/languages/definitions/ruby/register.js";
import "monaco-editor/esm/vs/languages/definitions/rust/register.js";
import "monaco-editor/esm/vs/languages/definitions/scss/register.js";
import "monaco-editor/esm/vs/languages/definitions/shell/register.js";
import "monaco-editor/esm/vs/languages/definitions/sql/register.js";
import "monaco-editor/esm/vs/languages/definitions/typescript/register.js";
import "monaco-editor/esm/vs/languages/definitions/xml/register.js";
import "monaco-editor/esm/vs/languages/definitions/yaml/register.js";

import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";

// JSON 在该定制包中没有独立 basic-languages 定义，这里注册语言并懒加载
// 无 worker 的 tokenizer，保证 JSON 高亮可用而不引入 JSON 语言服务。
monaco.languages.register({
  id: "json",
  extensions: [".json", ".bowerrc", ".jshintrc", ".jscsrc", ".eslintrc", ".babelrc", ".har"],
  aliases: ["JSON", "json"],
  mimetypes: ["application/json"],
});
monaco.languages.onLanguage("json", () => {
  void import("monaco-editor/esm/vs/languages/features/json/tokenization.js").then((mod) => {
    monaco.languages.setTokensProvider("json", mod.createTokenizationSupport(true));
  });
});

const monacoGlobal = globalThis as typeof globalThis & {
  MonacoEnvironment?: { getWorker: (moduleId: string, label: string) => Worker };
};

monacoGlobal.MonacoEnvironment = {
  getWorker() {
    return new EditorWorker();
  },
};

export { monaco };
