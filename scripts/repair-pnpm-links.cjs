// 修复 node_modules/.pnpm 内被建成空目录的依赖链接。
// 现象：pnpm 在本机创建部分链接时留下 0 项的空目录，导致 Node/TS 解析不到依赖。
// 做法：读取依赖方的 package.json 版本范围，在 .pnpm 中挑选匹配的存储目录，
// 用 junction 重建链接。只处理"空目录"这一种损坏形态，绝不覆盖已有内容。
const fs = require("fs");
const path = require("path");

const root = process.cwd();
const store = path.join(root, "node_modules", ".pnpm");

function parseVersion(entryName) {
  // rollup@4.62.3  |  vite@7.3.6_@types+node@26.5.1  |  @types+node@26.5.1
  const at = entryName.lastIndexOf("@");
  if (at <= 0) return null;
  const version = entryName.slice(at + 1).split("_")[0];
  return /^\d/.test(version) ? version : null;
}

function parseEntryName(entryName) {
  const at = entryName.lastIndexOf("@");
  if (at <= 0) return null;
  return { name: entryName.slice(0, at).replace(/\+/g, "/"), version: parseVersion(entryName) };
}

function cmp(a, b) {
  const pa = a.split(".").map((n) => parseInt(n, 10) || 0);
  const pb = b.split(".").map((n) => parseInt(n, 10) || 0);
  for (let i = 0; i < 3; i += 1) {
    if ((pa[i] || 0) !== (pb[i] || 0)) return (pa[i] || 0) - (pb[i] || 0);
  }
  return 0;
}

function satisfies(version, range) {
  if (!range || range === "*" || range === "latest") return true;
  const cleaned = range.trim().replace(/^[>=<^~]+/, "").split(" ")[0].split("||")[0].trim();
  if (!cleaned || cleaned === "*" || cleaned === "x") return true;
  const target = cleaned.replace(/^[>=<^~]+/, "");
  const base = target.split("-")[0];
  const parts = base.split(".").filter((p) => p !== "" && p !== "x" && p !== "*");
  if (!parts.length) return true;

  if (range.startsWith("^")) {
    if (cmp(version, base) < 0) return false;
    const major = parseInt(base.split(".")[0], 10);
    if (major === 0) {
      const minor = parseInt(base.split(".")[1] || "0", 10);
      return cmp(version, `0.${minor}.0`) >= 0 && cmp(version, `0.${minor + 1}.0`) < 0;
    }
    return cmp(version, `${major + 1}.0.0`) < 0;
  }
  if (range.startsWith("~")) {
    if (cmp(version, base) < 0) return false;
    const [major, minor] = base.split(".").map((n) => parseInt(n, 10) || 0);
    return cmp(version, `${major}.${minor + 1}.0`) < 0;
  }
  if (range.startsWith(">=")) return cmp(version, base) >= 0;
  if (range.startsWith(">")) return cmp(version, base) > 0;
  if (range.startsWith("<=")) return cmp(version, base) <= 0;
  if (range.startsWith("<")) return cmp(version, base) < 0;
  if (parts.length >= 3) return cmp(version, base) === 0;
  // x.y 或 x：视为前缀匹配
  return version.startsWith(parts.join("."));
}

const candidatesByName = new Map();
for (const entry of fs.readdirSync(store)) {
  const nm = path.join(store, entry, "node_modules");
  let names;
  try {
    names = fs.readdirSync(nm);
  } catch {
    continue;
  }
  for (const name of names) {
    // 作用域包要再下一层：@scope/pkg
    const pkgDirs = [];
    if (name.startsWith("@")) {
      const scopeDir = path.join(nm, name);
      let scopeNames;
      try {
        scopeNames = fs.readdirSync(scopeDir);
      } catch {
        continue;
      }
      for (const sub of scopeNames) pkgDirs.push({ dir: path.join(scopeDir, sub), full: `${name}/${sub}` });
    } else {
      pkgDirs.push({ dir: path.join(nm, name), full: name });
    }

    for (const candidate of pkgDirs) {
      // 只认"真实目录"，链接和空目录都不是候选包本体。
      let st;
      try {
        st = fs.lstatSync(candidate.dir);
      } catch {
        continue;
      }
      if (st.isSymbolicLink() || !st.isDirectory()) continue;
      const pkgJsonPath = path.join(candidate.dir, "package.json");
      if (!fs.existsSync(pkgJsonPath)) continue;
      let meta;
      try {
        meta = JSON.parse(fs.readFileSync(pkgJsonPath, "utf8"));
      } catch {
        continue;
      }
      // pnpm 会把超长包名截断成 name_hash，因此以 package.json 的 name 为准。
      if (!meta.name || !meta.version) continue;
      if (!candidatesByName.has(meta.name)) candidatesByName.set(meta.name, []);
      candidatesByName.get(meta.name).push({ entry, version: String(meta.version) });
    }
  }
}
for (const list of candidatesByName.values()) list.sort((a, b) => cmp(b.version, a.version));

function readDependentRanges(entryDir) {
  const ranges = new Map();
  const nm = path.join(entryDir, "node_modules");
  let names;
  try {
    names = fs.readdirSync(nm);
  } catch {
    return ranges;
  }
  for (const name of names) {
    // 主包目录（非链接）才代表这个 .pnpm 条目自身
    const pkgDir = path.join(nm, name);
    let st;
    try {
      st = fs.lstatSync(pkgDir);
    } catch {
      continue;
    }
    if (st.isSymbolicLink() || !st.isDirectory()) continue;
    const pkgJsonPath = path.join(pkgDir, "package.json");
    if (!fs.existsSync(pkgJsonPath)) continue;
    let pkgJson;
    try {
      pkgJson = JSON.parse(fs.readFileSync(pkgJsonPath, "utf8"));
    } catch {
      continue;
    }
    for (const field of [
      "dependencies",
      "optionalDependencies",
      "peerDependencies",
      "devDependencies",
    ]) {
      for (const [dep, range] of Object.entries(pkgJson[field] || {})) {
        if (typeof range === "string" && !ranges.has(dep)) ranges.set(dep, range);
      }
    }
  }
  return ranges;
}

let ok = 0;
const repaired = [];
const unresolved = [];

for (const entry of fs.readdirSync(store)) {
  const entryDir = path.join(store, entry);
  const nm = path.join(entryDir, "node_modules");
  let names;
  try {
    names = fs.readdirSync(nm);
  } catch {
    continue;
  }
  const ranges = readDependentRanges(entryDir);

  const targets = [];
  for (const name of names) {
    const p = path.join(nm, name);
    let st;
    try {
      st = fs.lstatSync(p);
    } catch {
      continue;
    }
    if (name.startsWith("@")) {
      if (!st.isDirectory() || st.isSymbolicLink()) continue;
      for (const sub of fs.readdirSync(p)) targets.push({ name: `${name}/${sub}`, linkPath: path.join(p, sub) });
    } else {
      targets.push({ name, linkPath: p });
    }
  }

  for (const target of targets) {
    let st;
    try {
      st = fs.lstatSync(target.linkPath);
    } catch {
      continue;
    }
    if (st.isSymbolicLink()) continue;
    if (!st.isDirectory()) continue;
    if (fs.readdirSync(target.linkPath).length !== 0) continue;

    const list = candidatesByName.get(target.name);
    if (!list || !list.length) {
      unresolved.push(`${target.linkPath} (no candidate)`);
      continue;
    }
    const range = ranges.get(target.name);
    let chosen = list.find((c) => satisfies(c.version, range)) || (list.length === 1 ? list[0] : null);
    if (!chosen) {
      unresolved.push(`${target.linkPath} (range=${range} candidates=${list.map((c) => c.version).join(",")})`);
      continue;
    }
    const resolved = path.join(store, chosen.entry, "node_modules", ...target.name.split("/"));
    if (!fs.existsSync(resolved) || fs.readdirSync(resolved).length === 0) {
      unresolved.push(`${target.linkPath} (target empty: ${chosen.entry})`);
      continue;
    }
    try {
      fs.rmdirSync(target.linkPath);
      fs.symlinkSync(path.resolve(resolved), target.linkPath, "junction");
      repaired.push(`${target.linkPath} -> ${chosen.entry}`);
    } catch (error) {
      unresolved.push(`${target.linkPath}: ${error.message}`);
    }
  }
}

console.log(`repaired=${repaired.length} unresolved=${unresolved.length}`);
for (const r of repaired) console.log("REPAIRED", r);
for (const u of unresolved) console.log("UNRESOLVED", u);
