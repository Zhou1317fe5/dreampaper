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

### Web UI

| 科研图 | 幻灯片 | 设置 |
| --- | --- | --- |
| ![figure ui](examples/ui/figure.jpg) | ![slide ui](examples/ui/slide.jpg) | ![settings ui](examples/ui/settings.jpg) |

### 科研绘图

|  |  |  |  |
| --- | --- | --- | --- |
| ![fig1](examples/figure/paper_figure_1.png) | ![fig2](examples/figure/paper_figure_2.png) | ![fig3](examples/figure/paper_figure_3.png) | ![fig4](examples/figure/paper_figure_4.png) |

### 幻灯片

|  |  |  |
| --- | --- | --- |
| ![sA1](examples/slide/slideA_1.png) | ![sA2](examples/slide/slideA_2.png) | ![sA3](examples/slide/slideA_3.png) |
| ![sB1](examples/slide/slideB_1.png) | ![sB2](examples/slide/slideB_2.png) | ![sB3](examples/slide/slideB_3.png) |
| ![sC1](examples/slide/slideC_1.png) | ![sC2](examples/slide/slideC_2.png) | ![sC3](examples/slide/slideC_3.png) |

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
