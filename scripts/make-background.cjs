// 生成内置会话背景图（Node 版，与 scripts/make-background.py 等价）。
// 输出 public/chat-background.png。
//
// 为什么需要这张图：旧版 chat-background.png 整体亮度 177-251，是"浅灰到白"的极浅图，
// 与白色面板几乎同色；叠加白纱与半透明面板后可见度只剩约 4%，视觉上等于没有背景。
// 本脚本生成明暗结构明确的背景（亮度约 130-250），确保在半透明面板下仍然看得出来。

const fs = require("fs");
const zlib = require("zlib");

const W = 1600;
const H = 900;

const clamp = (v) => Math.max(0, Math.min(255, Math.round(v)));
const smooth = (t) => t * t * (3 - 2 * t);
const lerp = (a, b, t) => a + (b - a) * t;

// 低饱和雾绿 / 灰蓝 / 暖砂，与绿色品牌色协调
const MIST = [238, 244, 241];
const SAGE = [182, 208, 195];
const DEEP = [110, 148, 132];
const SLATE = [134, 158, 178];
const SAND = [224, 210, 186];
const SHADOW = [88, 120, 112];

// (cx, cy, radius, color, strength) —— 归一化坐标
const BLOBS = [
  [0.16, 0.14, 0.46, DEEP, 1.0],
  [0.84, 0.08, 0.40, SLATE, 0.9],
  [0.04, 0.74, 0.44, SAGE, 0.95],
  [0.64, 0.88, 0.5, SHADOW, 0.92],
  [0.96, 0.56, 0.38, DEEP, 0.74],
  [0.44, 0.42, 0.32, MIST, 0.7],
  [0.3, 0.96, 0.34, SAND, 0.58],
];

const rows = [];
let lo = 255;
let hi = 0;
let tot = 0;
let n = 0;

for (let y = 0; y < H; y++) {
  const ny = y / H;
  const line = Buffer.alloc(1 + W * 3);
  line[0] = 0; // filter: none
  for (let x = 0; x < W; x++) {
    const nx = x / W;

    // 基底：自上而下由浅转深
    let r = lerp(232, 152, smooth(ny));
    let g = lerp(240, 176, smooth(ny));
    let b = lerp(235, 182, smooth(ny));

    // 斜向柔光带，打破纯渐变
    const band = smooth((Math.sin((nx * 1.7 + ny * 1.1) * Math.PI) + 1) / 2) * 26;
    r += band * 0.9;
    g += band * 1.0;
    b += band * 0.7;

    // 叠加大尺度光斑
    for (const [cx, cy, rad, col, strength] of BLOBS) {
      const dx = (nx - cx) * 1.6;
      const dy = ny - cy;
      const d = Math.sqrt(dx * dx + dy * dy) / rad;
      if (d >= 1) continue;
      const w = smooth(1 - d) * strength;
      r = lerp(r, col[0], w);
      g = lerp(g, col[1], w);
      b = lerp(b, col[2], w);
    }

    // 轻微颗粒，避免大面积纯色产生色带
    const nz = ((x * 7919 + y * 104729) % 13) - 6;
    const o = 1 + x * 3;
    line[o] = clamp(r + nz);
    line[o + 1] = clamp(g + nz);
    line[o + 2] = clamp(b + nz);

    if (x % 7 === 0 && y % 7 === 0) {
      const lum = (line[o] * 299 + line[o + 1] * 587 + line[o + 2] * 114) / 1000;
      if (lum < lo) lo = lum;
      if (lum > hi) hi = lum;
      tot += lum;
      n++;
    }
  }
  rows.push(line);
}

const raw = Buffer.concat(rows);

function chunk(tag, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const t = Buffer.from(tag, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(zlib.crc32 ? zlib.crc32(Buffer.concat([t, data])) : crc32(Buffer.concat([t, data])), 0);
  return Buffer.concat([len, t, data, crc]);
}

// 兼容旧版 Node 的 CRC32
function crc32(buf) {
  let c;
  const table = [];
  for (let k = 0; k < 256; k++) {
    c = k;
    for (let i = 0; i < 8; i++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[k] = c >>> 0;
  }
  let crc = 0xffffffff;
  for (let i = 0; i < buf.length; i++) crc = table[(crc ^ buf[i]) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(W, 0);
ihdr.writeUInt32BE(H, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 2; // color type: truecolor
ihdr[10] = 0;
ihdr[11] = 0;
ihdr[12] = 0;

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", zlib.deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

fs.writeFileSync("public/chat-background.png", png);

const report = [
  `dimensions = ${W}x${H}`,
  `file size = ${png.length} bytes`,
  `brightness min = ${Math.round(lo)}`,
  `brightness max = ${Math.round(hi)}`,
  `brightness spread = ${Math.round(hi - lo)}`,
  `brightness avg = ${Math.round(tot / n)}`,
].join("\n");

fs.writeFileSync("outputs/bg-report.txt", report + "\n", "utf8");
console.log(report);
