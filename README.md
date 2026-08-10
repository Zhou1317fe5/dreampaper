<p align="center">
  <img src="static/favor.png" width="112" alt="dreampaper logo">
</p>

<h1 align="center">DreamPaper</h1>

<p align="center"><strong>本地科研配图与学术幻灯片，从模板到成图一步到位。</strong></p>

<p align="center">
  <img alt="Python 3.10+" src="https://img.shields.io/badge/Python-3.10%2B-3776AB?logo=python&logoColor=white">
  <img alt="FastAPI 0.115+" src="https://img.shields.io/badge/FastAPI-0.115%2B-009688?logo=fastapi&logoColor=white">
  <img alt="React 19" src="https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black">
  <img alt="Vite 7" src="https://img.shields.io/badge/Vite-7-646CFF?logo=vite&logoColor=white">
  <img alt="TypeScript 5.8" src="https://img.shields.io/badge/TypeScript-5.8-3178C6?logo=typescript&logoColor=white">
  <img alt="Tauri 2" src="https://img.shields.io/badge/Tauri-2-24C8D5?logo=tauri&logoColor=white">
  <img alt="Rust stable" src="https://img.shields.io/badge/Rust-stable-000000?logo=rust&logoColor=white">
  <a href="LICENSE"><img alt="License PolyForm Noncommercial 1.0.0" src="https://img.shields.io/badge/License-PolyForm%20Noncommercial%201.0.0-F5A623"></a>
</p>

<p align="center">English: <a href="README_EN.md">README_EN.md</a></p>

---

## 核心优势

- **模板驱动**：科研图以 PaperBananaBench 参考图作 few-shot；幻灯片以你上传的母版锁定版式与配色
- **两阶段设计**：先抽结构 / 母版，再填内容，减少「抄模板文案」与样式漂移
- **模型自选**：Design / Implement / Search 分配置，兼容 OpenAI / Anthropic / image2 / banana2 等协议
- **幻灯片视觉 grounding**：从资料中识别产品与仪器，检索外观描述，引导制图模型画实物而非文字方框
- **全程本地**：配置与产物落在 `~/.dreampaper/`，密钥不进仓库

---

## 效果展示

### Desktop

| 科研图 | 幻灯片 |
| --- | --- | 
| ![desktop1](examples/desktop/figure.jpg) | ![desktop2](examples/desktop/slide.jpg) |

| 模版库 | 设置页 |
| --- | --- | 
| ![desktop3](examples/desktop/templates.jpg) | ![fig2](examples/desktop/settings.jpg) |

### Web UI

| 科研图 | 幻灯片 | 设置页 |
| --- | --- | --- |
| ![figure ui](examples/ui/figure.jpg) | ![slide ui](examples/ui/slide.jpg) | ![settings ui](examples/ui/settings.jpg) |

### 科研绘图

|  |  |
| --- | --- | 
| ![fig1](examples/figure/paper_figure_1.png) | ![fig2](examples/figure/paper_figure_2.png) |
| ![fig3](examples/figure/paper_figure_3.png) | ![fig4](examples/figure/paper_figure_4.png) |
| ![fig5](examples/figure/paper_figure_5.png) | ![fig6](examples/figure/paper_figure_6.png) |

### 幻灯片

|  |  |  |
| --- | --- | --- |
| ![sA1](examples/slide/slideA_1.png) | ![sA2](examples/slide/slideA_2.png) | ![sA3](examples/slide/slideA_3.png) |
| ![sB1](examples/slide/slideB_1.png) | ![sB2](examples/slide/slideB_2.png) | ![sB3](examples/slide/slideB_3.png) |
| ![sC1](examples/slide/slideC_1.png) | ![sC2](examples/slide/slideC_2.png) | ![sC3](examples/slide/slideC_3.png) |
| ![sD1](examples/slide/slideD_1.png) | ![sD2](examples/slide/slideD_2.png) | ![sD3](examples/slide/slideD_3.png) |
---

## 桌面版下载

不想配 Python 环境的话，直接下[最新 Release](../../releases/latest)。桌面版内置 Rust 后端，无需单独启动服务。

| 文件 | 平台 |
| --- | --- |
| `dreampaper-*-setup.exe` | Windows 安装版（创建桌面快捷方式） |
| `dreampaper-*-portable.exe` | Windows 便携版（免安装） |
| `dreampaper-*-x64-mac.dmg` | macOS Intel |
| `dreampaper-*-arm64-mac.dmg` | macOS Apple Silicon |

### 首次打开

安装包**未做代码签名**（未购买开发者证书），系统会拦一次：

- **macOS**：双击提示「无法验证开发者」。右键点 App → 选「打开」→ 再点一次「打开」。只需操作一次。
- **Windows**：SmartScreen 提示「已保护你的电脑」。点「更多信息」→「仍要运行」。
- **Windows 便携版**依赖系统的 WebView2 运行时。Win11 与 Win10 21H2 及以上已内置；更旧的系统请先装
  [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)，或改用安装版（会自动处理）。

桌面版的配置与产物落在系统应用数据目录，而非 `~/.dreampaper/`：

| 平台 | 路径 |
| --- | --- |
| macOS | `~/Library/Application Support/com.dreampaper.app/` |
| Windows | `%APPDATA%\com.dreampaper.app\` |

---

## 准备 Template 库（科研图）

本仓库**不附带** PaperBananaBench。科研图模式需要本地参考图库。

1. 下载 [PaperBananaBench](https://huggingface.co/datasets/dwzhu/PaperBananaBench)（约 266MB）
2. 解压到仓库根目录，结构如下：

```text
dream-paper/
  PaperBananaBench/
    diagram/
      ref.json
      images/…
    plot/
      ref.json
      images/…
```

3. 重启后端后，科研图页即可选择 1–3 张 template

> 仅用幻灯片模式时可不下载。

**桌面版**不读仓库目录，改在「模板库」页导入：

- **导入模板包**：解压 PaperBananaBench 后，点「选择目录…」选中解压出的那个目录（内含 `diagram/` 与 `plot/`），一次导入全部
- **导入图片**：单张导入自有模板，填类型 / 分类 / 描述即可

导入后的模板复制到应用数据目录，与仓库解耦，换分支或删仓库都不影响。

---

## 本地启动

```bash
python3 -m venv .venv
source .venv/bin/activate   # Windows: .venv\Scripts\activate
pip install -r requirements.txt
npm install
```

```bash
# 终端 1 — 后端
uvicorn backend.app.main:app --reload --host 127.0.0.1 --port 8000

# 终端 2 — 前端
npm run dev
```

打开 http://127.0.0.1:5173

---

## 使用

1. **设置**：配置 Design / Implement（及可选 Search、代理、并发），保存  
2. **科研图**：选 template → 填标题与方法 → 生成  
3. **幻灯片**：上传母版图 → 填资料与页数 → 生成  

| 路径 | 内容 |
| --- | --- |
| `~/.dreampaper/config.json` | 模型配置 |
| `~/.dreampaper/assets/` | 上传文件 |
| `~/.dreampaper/jobs/` | 任务记录与出图 |

---

## 致谢

科研图 template 来自 [PaperBananaBench](https://huggingface.co/datasets/dwzhu/PaperBananaBench)（[PaperBanana](https://github.com/dwzhu-pku/PaperBanana)）。

学AI,上L站。Ref: https://linux.do/

---

## 开源协议

本项目采用 [PolyForm Noncommercial License 1.0.0](LICENSE)，**允许非商业使用，禁止商业使用**。

| 用途 | 是否允许 |
| --- | --- |
| 个人学习、研究、实验、业余项目 | ✅ |
| 阅读、修改源码，二次开发与分发 | ✅（需附带本协议） |
| 公司内部生产使用、对外提供付费或商业服务 | ❌ |
| 将本项目或其衍生版本作为商品出售 | ❌ |
