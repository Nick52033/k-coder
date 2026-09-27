// Test-only resolver for plugin fixtures.
//
// k-Coder plugins live in two scopes: the machine-wide local plugin root under the
// application data directory (shared by every project) and the per-project root at
// `<workspace>/.k-coder/plugins`. The repository deliberately ships no plugin content,
// so a test that needs a plugin fixture resolves it from either root and skips itself
// with an explicit reason when neither root has that plugin installed.
import os from "node:os";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const workspace = path.resolve(import.meta.dirname, "..");

function localPluginRoots() {
  const roots = [];
  if (process.env.KCODER_DATA_ROOT) {
    roots.push(path.join(process.env.KCODER_DATA_ROOT, "plugins"));
  }
  if (process.env.APPDATA) {
    roots.push(path.join(process.env.APPDATA, "com.kcoder.app", "runtime-data", "plugins"));
  }
  return roots;
}

/** Absolute root of an installed plugin, or `null` when it is not available. */
export function findPluginRoot(name) {
  const candidates = [
    path.join(workspace, ".k-coder", "plugins", name),
    ...localPluginRoots().map((root) => path.join(root, name)),
  ];
  return candidates.find((candidate) => fs.existsSync(candidate)) ?? null;
}

/** Absolute path to one file inside an installed plugin, or `null` when unavailable. */
export function findPluginFile(name, ...relativePath) {
  const root = findPluginRoot(name);
  return root ? path.join(root, ...relativePath) : null;
}

/** `node:test` skip option naming the plugin a case depends on. */
export function skipWithoutPlugin(name) {
  return findPluginRoot(name) ? false : `${name} plugin is not installed in the local or project plugin root`;
}

/** `file://` URL for one file inside an installed plugin, for Windows-safe `import()`. */
export function pluginFileUrl(name, ...relativePath) {
  const resolved = findPluginFile(name, ...relativePath);
  return resolved ? pathToFileURL(resolved).href : null;
}

/**
 * Run `callback` with the artifact-tool module directory exported the same way
 * `scripts/plugin-artifact-runtime.ps1` does, so a helper that now lives outside the
 * workspace can still resolve `@oai/artifact-tool` without workspace dependency links.
 */
export async function withArtifactRuntime(callback) {
  const modules = path.join(
    os.homedir(),
    ".cache",
    "codex-runtimes",
    "codex-primary-runtime",
    "dependencies",
    "node",
    "node_modules",
  );
  if (!fs.existsSync(path.join(modules, "@oai", "artifact-tool"))) return false;
  const previous = process.env.K_CODER_ARTIFACT_NODE_MODULES;
  process.env.K_CODER_ARTIFACT_NODE_MODULES = modules;
  try {
    return await callback();
  } finally {
    if (previous === undefined) delete process.env.K_CODER_ARTIFACT_NODE_MODULES;
    else process.env.K_CODER_ARTIFACT_NODE_MODULES = previous;
  }
}
