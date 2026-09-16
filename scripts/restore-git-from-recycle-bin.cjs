// 从回收站恢复 D:\code\k-coder\.git 下被删除的文件。
// 只做复制，不动回收站内容，保证可重复执行、可回退。
const fs = require("fs");
const path = require("path");

const BIN = "D:/$RECYCLE.BIN/S-1-5-21-1724528068-1882292782-1657361363-401312";
const PREFIX = "D:\\code\\k-coder\\.git\\";
const SKIP = /\.(lock)$|index\.stash\./i;

function parseInfo(file) {
  const b = fs.readFileSync(file);
  const len = b.readUInt32LE(24);
  return b.slice(28, 28 + len * 2).toString("utf16le").replace(/\0+$/, "");
}

const entries = [];
for (const name of fs.readdirSync(BIN)) {
  if (!name.startsWith("$I")) continue;
  let original;
  try {
    original = parseInfo(path.join(BIN, name));
  } catch {
    continue;
  }
  if (!original.startsWith(PREFIX)) continue;
  if (SKIP.test(original)) continue;
  entries.push({ name, original, data: "$R" + name.slice(2) });
}

// 先恢复文件，再补目录；浅层优先，保证父目录先存在。
entries.sort((a, b) => a.original.length - b.original.length);

let files = 0;
let dirs = 0;
const failures = [];
for (const entry of entries) {
  const src = path.join(BIN, entry.data);
  const dest = entry.original;
  try {
    const st = fs.statSync(src);
    if (st.isDirectory()) {
      fs.mkdirSync(dest, { recursive: true });
      dirs += 1;
      continue;
    }
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    if (fs.existsSync(dest) && fs.statSync(dest).size === st.size) continue;
    fs.copyFileSync(src, dest);
    files += 1;
  } catch (error) {
    failures.push(`${dest}: ${error.message}`);
  }
}

console.log(`entries=${entries.length} files=${files} dirs=${dirs} failures=${failures.length}`);
for (const f of failures.slice(0, 20)) console.log("FAIL", f);
