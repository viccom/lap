#!/usr/bin/env bash
# F0 Rust 构建环境：注入 VS BuildTools 的 cmake 与 MSVC 工具链路径。
# VS 安装路径由 vswhere 动态探测，不写死版本目录。
set -uo pipefail

VSWHERE="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
VS_PATH=$("$VSWHERE" -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>/dev/null | tr -d '\r')
if [ -z "$VS_PATH" ]; then
  echo "FATAL: 未找到 MSVC Build Tools" >&2
  exit 1
fi
# 转成 POSIX 路径
VS_POSIX=$(cygpath -u "$VS_PATH")

CMAKE_BIN="$VS_POSIX/Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin"
# 找 MSVC 工具链 bin（版本目录动态取最新）
MSVC_ROOT=$(find "$VS_POSIX/VC/Tools/MSVC" -maxdepth 1 -mindepth 1 -type d 2>/dev/null | sort -V | tail -1)
MSVC_BIN="$MSVC_ROOT/bin/Hostx64/x64"

# Windows SDK（lib/link/rc）：直接定位 rc.exe 所在目录
SDK_BIN=$(dirname "$(find "/c/Program Files (x86)/Windows Kits/10/bin" -iname "rc.exe" -path "*x64*" 2>/dev/null | sort -V | tail -1)")
if [ -z "$SDK_BIN" ]; then SDK_BIN=""; fi

export PATH="$CMAKE_BIN:$MSVC_BIN:$SDK_BIN:/c/ProgramData/scoop/shims:$PATH"

echo "VS      : $VS_PATH"
echo "cmake  : $(command -v cmake || echo MISSING)"
echo "nasm   : $(command -v nasm || echo MISSING)"
echo "cl     : $(command -v cl.exe || echo MISSING)"
echo "link   : $(command -v link.exe || echo MISSING)"
echo "rc     : $(command -v rc.exe || echo MISSING)"
