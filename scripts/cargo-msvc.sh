#!/usr/bin/env bash
# 在 MSVC 环境下运行 cargo。
#
# 为什么需要这个包装脚本：
# 1. 本机 Git Bash 的 `/usr/bin/link`（GNU coreutils 的 link）会遮蔽 MSVC 的
#    `link.exe`，直接 cargo 会以 `link: missing operand after '\377\376'` 失败；
# 2. 即使 PATH 修好，`link.exe` 仍找不到 Windows SDK 的导入库，报
#    `LNK1181: 无法打开输入文件 OleAut32.lib / kernel32.lib`；
# 3. 本环境的 Bash 安全策略禁止调用 `cmd.exe`，所以不能用常规的
#    `vcvars64.bat` 方案。
#
# 因此这里直接把 MSVC 与 Windows SDK 的 bin 目录前置到 PATH，并显式设置 LIB。
# 路径写成 Windows 形式（反斜杠 + `;` 分隔），由 Git Bash 自行转换。
#
# 工具链位置**逐个候选探测**而不是写死：本机 C 盘曾经满到 0 字节，Windows SDK 被整体
# 挪到了 `D:\Windows Kits\10`，写死 `C:\Program Files (x86)\Windows Kits\10` 会让脚本
# 在 `ls` 那一步静默退出（`set -e`），表现为「跑 5 秒、没有任何 cargo 输出、退出码 0」。
#
# 用法：scripts/cargo-msvc.sh check --lib
#       scripts/cargo-msvc.sh test --lib mobile::projects
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 每个候选都写成 "unix 路径|windows 路径" 两条，避免再做盘符转换。
find_root() {
  local label="$1"
  shift
  local entry unix_path
  for entry in "$@"; do
    unix_path="${entry%%|*}"
    if [ -d "${unix_path}" ]; then
      printf '%s\n' "${entry}"
      return 0
    fi
  done
  echo "cargo-msvc.sh: 找不到 ${label}，已尝试：" >&2
  printf '  - %s\n' "${@%%|*}" >&2
  return 1
}

VS_ENTRY="$(find_root "Visual Studio 2022 工具链" \
  "/d/Program Files/Microsoft Visual Studio/2022/Community|D:\\Program Files\\Microsoft Visual Studio\\2022\\Community" \
  "/c/Program Files/Microsoft Visual Studio/2022/Community|C:\\Program Files\\Microsoft Visual Studio\\2022\\Community")"
VS_ROOT="${VS_ENTRY%%|*}"
VS="${VS_ENTRY##*|}"
MSVC_VER="$(ls -1 "${VS_ROOT}/VC/Tools/MSVC" | sort -V | tail -1)"

SDK_ENTRY="$(find_root "Windows 10/11 SDK（需要 Lib 目录）" \
  "/c/Program Files (x86)/Windows Kits/10|C:\\Program Files (x86)\\Windows Kits\\10" \
  "/d/Windows Kits/10|D:\\Windows Kits\\10" \
  "/c/Program Files/Windows Kits/10|C:\\Program Files\\Windows Kits\\10")"
SDK_ROOT="${SDK_ENTRY%%|*}"
SDK_ROOT_WIN="${SDK_ENTRY##*|}"
SDK_VER="$(ls -1 "${SDK_ROOT}/Lib" | sort -V | tail -1)"

MSVC_BIN="${VS_ROOT}/VC/Tools/MSVC/${MSVC_VER}/bin/Hostx64/x64"
SDK_BIN="${SDK_ROOT}/bin/${SDK_VER}/x64"

# MSVC 的 bin 必须排在 `/usr/bin` 之前，否则 link/rc 依旧被 Git Bash 的同名程序遮蔽。
export PATH="${MSVC_BIN}:${SDK_BIN}:${PATH}"

export LIB="${VS}\\VC\\Tools\\MSVC\\${MSVC_VER}\\lib\\x64;"
export LIB="${LIB}${SDK_ROOT_WIN}\\Lib\\${SDK_VER}\\um\\x64;"
export LIB="${LIB}${SDK_ROOT_WIN}\\Lib\\${SDK_VER}\\ucrt\\x64"

export INCLUDE="${VS}\\VC\\Tools\\MSVC\\${MSVC_VER}\\include;"
export INCLUDE="${INCLUDE}${SDK_ROOT_WIN}\\Include\\${SDK_VER}\\um;"
export INCLUDE="${INCLUDE}${SDK_ROOT_WIN}\\Include\\${SDK_VER}\\ucrt;"
export INCLUDE="${INCLUDE}${SDK_ROOT_WIN}\\Include\\${SDK_VER}\\shared"

cd "${REPO_ROOT}/src-tauri"
exec cargo "$@"
