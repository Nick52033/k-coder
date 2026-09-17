"""生成内置会话背景图。

设计目标：让背景图在半透明面板之下仍然"看得出来"。

背景：旧版 chat-background.png 整体亮度 177-251，是一张"浅灰到白"的极浅图，
与白色面板几乎同色；再叠加 30% 白纱 + 74%~88% 面板后，可见度只剩约 4%，
所以视觉上等于"没有背景图"。这是原图本身的问题，不是透明度参数能救的。

本脚本生成的背景具备：
  - 明确的明暗结构（亮度约 140-240），与白色面板拉开可见差异
  - 大尺度柔和色块，缩小到窗口尺寸后仍能辨认出结构
  - 低饱和配色，与 k-Coder 的绿色品牌色协调，不干扰正文阅读
"""

import math
import struct
import zlib

W, H = 1600, 900


def clamp(v, lo=0, hi=255):
    return max(lo, min(hi, int(v)))


def smooth(t):
    return t * t * (3 - 2 * t)


def lerp(a, b, t):
    return a + (b - a) * t


# 低饱和的雾绿 / 灰蓝 / 暖砂，与绿色品牌色相处融洽
MIST = (236, 242, 239)
SAGE = (188, 210, 199)
DEEP = (118, 152, 138)
SLATE = (140, 162, 180)
SAND = (222, 210, 188)
SHADOW = (96, 124, 117)


def blobs():
    """大尺度柔光斑：决定整体明暗结构。"""
    return [
        # (cx, cy, radius, color, strength)
        (0.16, 0.14, 0.46, DEEP, 0.98),
        (0.84, 0.08, 0.40, SLATE, 0.86),
        (0.04, 0.74, 0.44, SAGE, 0.92),
        (0.64, 0.88, 0.50, SHADOW, 0.90),
        (0.96, 0.56, 0.38, DEEP, 0.72),
        (0.44, 0.42, 0.32, MIST, 0.68),
        (0.30, 0.96, 0.34, SAND, 0.56),
    ]


def build():
    bl = blobs()
    rows = []
    for y in range(H):
        ny = y / H
        row = bytearray()
        for x in range(W):
            nx = x / W

            # 基底：自上而下由浅转深
            r = lerp(230, 158, smooth(ny))
            g = lerp(238, 180, smooth(ny))
            b = lerp(233, 186, smooth(ny))

            # 斜向柔光带，打破纯渐变
            band = math.sin((nx * 1.7 + ny * 1.1) * math.pi) * 0.5 + 0.5
            band = smooth(band) * 24
            r += band * 0.9
            g += band * 1.0
            b += band * 0.7

            # 叠加光斑
            for cx, cy, rad, color, strength in bl:
                dx = (nx - cx) * 1.6
                dy = ny - cy
                d = math.sqrt(dx * dx + dy * dy) / rad
                if d >= 1.0:
                    continue
                w = smooth(1.0 - d) * strength
                r = lerp(r, color[0], w)
                g = lerp(g, color[1], w)
                b = lerp(b, color[2], w)

            # 轻微颗粒，避免大面积纯色产生色带
            n = ((x * 7919 + y * 104729) % 13) - 6
            row += bytes((clamp(r + n), clamp(g + n), clamp(b + n)))
        rows.append(row)
    return rows


def write_png(path, rows):
    raw = b"".join(b"\x00" + bytes(r) for r in rows)

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


if __name__ == "__main__":
    rows = build()
    out = "public/chat-background.png"
    write_png(out, rows)

    lo, hi, tot, n = 255, 0, 0, 0
    for y in range(0, H, 9):
        r = rows[y]
        for x in range(0, W, 9):
            o = x * 3
            lum = (r[o] * 299 + r[o + 1] * 587 + r[o + 2] * 114) // 1000
            lo, hi = min(lo, lum), max(hi, lum)
            tot += lum
            n += 1
    print(f"已生成 {out}  {W}x{H}")
    print(f"亮度范围 {lo}-{hi}  变化幅度 {hi - lo}  平均 {tot // n}")
