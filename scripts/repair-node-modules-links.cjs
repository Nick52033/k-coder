// 修复 pnpm 隔离式 node_modules 中缺失/空的顶层依赖链接。
// 背景：部分直接依赖在 node_modules/<name> 处留下空目录，导致 TS/Vite 解析失败。
// 本脚本按 package.json 的 dependencies/devDependencies 逐个校验，必要时用
// .pnpm 存储目录重建 junction。只补不删。
const fs = require("fs");
const path = require("path");

const root = process.cwd();
const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
const names = [
  ...Object.keys(pkg.dependencies || {}),
  ...Object.keys(pkg.devDependencies || {}),
];

const storeDir = path.join(root, "node_modules", ".pnpm");

/** 在 .pnpm 中查找该包名对应的存储目录（可能有多个版本，取第一个）。 */
function findInStore(name) {
  const encoded = name.replace("/", "+");
  const prefix = `${encoded}@`;
  let entries;
  try {
    entries = fs.readdirSync(storeDir);
  } catch {
    return null;
  }
  const candidates = entries
    .filter((entry) => entry === encoded || entry.startsWith(prefix))
    .sort()
    .reverse();
  for (const candidate of candidates) {
    const target = path.join(storeDir, candidate, "node_modules", name);
    if (fs.existsSync(target) && fs.readdirSync(target).length > 0) return target;
  }
  return null;
}

function isEmptyDir(p) {
  try {
    const st = fs.lstatSync(p);
    if (!st.isDirectory() && !st.isSymbolicLink()) return false;
    return fs.readdirSync(p).length === 0;
  } catch {
    return false;
  }
}

let ok = 0;
const repaired = [];
const stillBroken = [];

for (const name of names) {
  const linkPath = path.join(root, "node_modules", ...name.split("/"));
  const exists = fs.existsSync(linkPath);
  if (exists && !isEmptyDir(linkPath)) {
    ok += 1;
    continue;
  }
  const target = findInStore(name);
  if (!target) {
    stillBroken.push(`${name} (no store entry)`);
    continue;
  }
  try {
    fs.mkdirSync(path.dirname(linkPath), { recursive: true });
    if (fs.existsSync(linkPath)) fs.rmSync(linkPath, { recursive: true, force: true });
    fs.symlinkSync(target, linkPath, "junction");
    repaired.push(`${name} -> ${path.relative(root, target)}`);
  } catch (error) {
    stillBroken.push(`${name}: ${error.message}`);
  }
}

console.log(`checked=${names.length} ok=${ok} repaired=${repaired.length} broken=${stillBroken.length}`);
for (const r of repaired) console.log("REPAIRED", r);
for (const b of stillBroken) console.log("BROKEN  ", b);
