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
# 用法：scripts/cargo-msvc.sh check --lib
#       scripts/cargo-msvc.sh test --lib mobile::projects
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

VS="D:\\Program Files\\Microsoft Visual Studio\\2022\\Community"
MSVC_VER="$(ls -1 "/d/Program Files/Microsoft Visual Studio/2022/Community/VC/Tools/MSVC" | sort -V | tail -1)"
SDK_VER="$(ls -1 "/c/Program Files (x86)/Windows Kits/10/Lib" | sort -V | tail -1)"

MSVC_BIN="/d/Program Files/Microsoft Visual Studio/2022/Community/VC/Tools/MSVC/${MSVC_VER}/bin/Hostx64/x64"
SDK_BIN="/c/Program Files (x86)/Windows Kits/10/bin/${SDK_VER}/x64"

# MSVC 的 bin 必须排在 `/usr/bin` 之前，否则 link/rc 依旧被 Git Bash 的同名程序遮蔽。
export PATH="${MSVC_BIN}:${SDK_BIN}:${PATH}"

export LIB="${VS}\\VC\\Tools\\MSVC\\${MSVC_VER}\\lib\\x64;"
export LIB="${LIB}C:\\Program Files (x86)\\Windows Kits\\10\\Lib\\${SDK_VER}\\um\\x64;"
export LIB="${LIB}C:\\Program Files (x86)\\Windows Kits\\10\\Lib\\${SDK_VER}\\ucrt\\x64"

export INCLUDE="${VS}\\VC\\Tools\\MSVC\\${MSVC_VER}\\include;"
export INCLUDE="${INCLUDE}C:\\Program Files (x86)\\Windows Kits\\10\\Include\\${SDK_VER}\\um;"
export INCLUDE="${INCLUDE}C:\\Program Files (x86)\\Windows Kits\\10\\Include\\${SDK_VER}\\ucrt;"
export INCLUDE="${INCLUDE}C:\\Program Files (x86)\\Windows Kits\\10\\Include\\${SDK_VER}\\shared"

cd "${REPO_ROOT}/src-tauri"
exec cargo "$@"
