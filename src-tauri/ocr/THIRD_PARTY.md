# 第三方来源说明（`dreampaper-ocr`）

本 crate 为 DreamPaper 工作台的纯 Rust OCR 辅助进程。以下组件按其许可证条款使用，随应用分发时需保留对应声明。

## 代码

| 组件 | 版本 / 来源 | 许可证 | 使用方式 |
| --- | --- | --- | --- |
| `ppocr-rs`（Dario Finardi） | 0.7.3，<https://crates.io/crates/ppocr-rs> | Apache-2.0 | `src/ppocr/**` 为其 det/cls/rec 预后处理的受控移植；已去除下载、执行器特性、版面/表格/公式模块，并按 RapidOCR 3.9.2 / PaddleX 参考实现修正 BGR 通道顺序、cls 补白、`score_mode fast` 与坐标剪裁 |
| `paddle-ocr-rs`（meibel-ai） | `ppocr-rs` 的上游 | Apache-2.0 | 同上，间接来源 |
| `ort` | 2.0.0-rc.9 | MIT / Apache-2.0 | ONNX Runtime 动态加载绑定（`load-dynamic`） |
| ONNX Runtime | 1.29.0（tag `v1.29.0`，commit `2e2543f`），由 `scripts/ort.mjs` 自构建 CPU 版 | MIT | 随安装包分发的动态库；构建脚本对 AppleClang 14 做了 3 处仅影响编译的源码兼容替换 |
| `image` / `imageproc` / `ndarray` / `geo-clipper` / `geo-types` / `serde` / `thiserror` | 见 `Cargo.lock` | MIT / Apache-2.0 | 图像处理、几何偏移、序列化 |

## 模型与字典（可选组件，按需下载，不随安装包分发）

| 文件 | 来源 | 许可证 |
| --- | --- | --- |
| `det.onnx` / `rec.onnx` | PaddleOCR 官方 `PP-OCRv6_medium_{det,rec}_onnx_infer.tar`（`paddle-model-ecology.bj.bcebos.com`） | Apache-2.0 |
| `cls.onnx` | RapidOCR v3.9.2 分发的 `ch_ppocr_mobile_v2.0_cls_mobile.onnx`（ModelScope `RapidAI/RapidOCR`） | Apache-2.0 |
| `dict.txt` | 由 `rec` 包内 `inference.yml` 的 `PostProcess.character_dict` 逐行导出 | 随模型 Apache-2.0 |

精确字节数与 SHA-256 见 `manifest.json`（gate 校验）与 `src-tauri/src/core/workbench/manifest.json`（应用安装校验）。
