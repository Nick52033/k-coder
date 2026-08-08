// monaco-editor 的 esm/vs/languages/features/json/tokenization.js 未随包发布 .d.ts，
// 这里按 register.d.ts 中对应 API 的签名补充声明，供定制装配按需导入。
declare module "monaco-editor/esm/vs/languages/features/json/tokenization.js" {
  import type { languages } from "monaco-editor";

  export function createTokenizationSupport(onlyJson: boolean): languages.TokensProvider;
}
