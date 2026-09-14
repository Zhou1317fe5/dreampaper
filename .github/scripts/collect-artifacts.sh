#!/usr/bin/env bash
set -euo pipefail
node scripts/bundle.mjs "${1:?缺少 rust target 参数}" "${2:?缺少版本号参数}"
