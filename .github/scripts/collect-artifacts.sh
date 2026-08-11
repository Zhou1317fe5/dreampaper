#!/usr/bin/env bash

set -euo pipefail

target="${1:?缺少 rust target 参数}"
version="${2:?缺少版本号参数}"

bundle_dir="src-tauri/target/${target}/release/bundle"
binary_dir="src-tauri/target/${target}/release"
out_dir="release-artifacts"

mkdir -p "$out_dir"

take_one() {
  local description="$1"
  shift
  local matches=()
  while IFS= read -r line; do
    [ -n "$line" ] && matches+=("$line")
  done < <(find "$@" 2>/dev/null | sort)

  if [ "${#matches[@]}" -eq 0 ]; then
    echo "错误：找不到${description}。查找条件：$*" >&2
    echo "--- 现有产物 ---" >&2
    find "src-tauri/target/${target}/release" -maxdepth 3 \
      \( -name '*.exe' -o -name '*.dmg' -o -name '*.app' \) 2>/dev/null >&2 || true
    exit 1
  fi
  if [ "${#matches[@]}" -gt 1 ]; then
    echo "错误：${description}匹配到多个文件，无法确定该发哪个：" >&2
    printf '  %s\n' "${matches[@]}" >&2
    exit 1
  fi
  printf '%s' "${matches[0]}"
}

emit() {
  local src="$1" dest="$2"
  cp "$src" "$out_dir/$dest"
  echo "$src → $out_dir/$dest"
}

case "$target" in
  *windows*)
    setup="$(take_one "NSIS 安装包" "$bundle_dir/nsis" -maxdepth 1 -name '*-setup.exe')"
    emit "$setup" "dreampaper-${version}-setup.exe"

    portable="$(take_one "便携版可执行文件" "$binary_dir" -maxdepth 1 -iname 'dreampaper.exe')"
    emit "$portable" "dreampaper-${version}-portable.exe"
    ;;
  x86_64-apple-darwin)
    dmg="$(take_one "Intel dmg" "$bundle_dir/dmg" -maxdepth 1 -name '*.dmg')"
    emit "$dmg" "dreampaper-${version}-x64-mac.dmg"
    ;;
  aarch64-apple-darwin)
    dmg="$(take_one "Apple Silicon dmg" "$bundle_dir/dmg" -maxdepth 1 -name '*.dmg')"
    emit "$dmg" "dreampaper-${version}-arm64-mac.dmg"
    ;;
  *)
    echo "错误：未支持的 target $target" >&2
    exit 1
    ;;
esac

echo "--- 待发布产物 ---"
ls -lh "$out_dir"
