<p align="center">
  <img src="static/favor.png" width="112" alt="dreampaper logo">
</p>

<h1 align="center">DreamPaper</h1>

<p align="center"><strong>Local paper figures and academic slides — template in, publication-ready image out.</strong></p>

<p align="center">
  <img alt="Python 3.10+" src="https://img.shields.io/badge/Python-3.10%2B-3776AB?logo=python&logoColor=white">
  <img alt="FastAPI 0.115+" src="https://img.shields.io/badge/FastAPI-0.115%2B-009688?logo=fastapi&logoColor=white">
  <img alt="React 19" src="https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black">
  <img alt="Vite 7" src="https://img.shields.io/badge/Vite-7-646CFF?logo=vite&logoColor=white">
  <img alt="TypeScript 5.8" src="https://img.shields.io/badge/TypeScript-5.8-3178C6?logo=typescript&logoColor=white">
</p>

<p align="center">中文版：<a href="README.md">README.md</a></p>

---

## Why dreampaper

- **Template-driven** — figures use PaperBananaBench few-shot refs; slides lock layout and palette from your master image
- **Two-stage design** — structure / master first, then content, less template-copy and style drift
- **Bring your own models** — separate Design / Implement / Search profiles (OpenAI, Anthropic, image2, banana2, …)
- **Slide visual grounding** — detect products and instruments in material, search appearance cues, draw real objects instead of labeled boxes
- **Fully local** — config and outputs under `~/.dreampaper/`; keys never enter the repo

---

## Gallery

### Web UI

| Figure | Slide | Settings |
| --- | --- | --- |
| ![figure ui](examples/ui/figure.jpg) | ![slide ui](examples/ui/slide.jpg) | ![settings ui](examples/ui/settings.jpg) |

### Paper figures

|  |  |  |  |
| --- | --- | --- | --- |
| ![fig1](examples/figure/paper_figure_1.png) | ![fig2](examples/figure/paper_figure_2.png) | ![fig3](examples/figure/paper_figure_3.png) | ![fig4](examples/figure/paper_figure_4.png) |

### Slides

|  |  |  |
| --- | --- | --- |
| ![sA1](examples/slide/slideA_1.png) | ![sA2](examples/slide/slideA_2.png) | ![sA3](examples/slide/slideA_3.png) |
| ![sB1](examples/slide/slideB_1.png) | ![sB2](examples/slide/slideB_2.png) | ![sB3](examples/slide/slideB_3.png) |
| ![sC1](examples/slide/slideC_1.png) | ![sC2](examples/slide/slideC_2.png) | ![sC3](examples/slide/slideC_3.png) |

---

## Template library (paper figures)

This repo **does not ship** PaperBananaBench. Figure mode needs it locally.

1. Download [PaperBananaBench](https://huggingface.co/datasets/dwzhu/PaperBananaBench) (~266MB)
2. Unzip at the repo root:

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

3. Restart the backend, then pick 1–3 templates in the Figure page

> Skip this if you only use Slide mode.

---

## Run locally

```bash
python3 -m venv .venv
source .venv/bin/activate   # Windows: .venv\Scripts\activate
pip install -r requirements.txt
npm install
```

```bash
# terminal 1 — API
uvicorn backend.app.main:app --reload --host 127.0.0.1 --port 8000

# terminal 2 — UI
npm run dev
```

Open http://127.0.0.1:5173

---

## Usage

1. **Settings** — configure Design / Implement (optional Search, proxy, concurrency), save  
2. **Figure** — pick templates → title + method → generate  
3. **Slide** — upload master → material + page count → generate  

| Path | Content |
| --- | --- |
| `~/.dreampaper/config.json` | model config |
| `~/.dreampaper/assets/` | uploads |
| `~/.dreampaper/jobs/` | job logs and images |

---

## Credits

Figure templates from [PaperBananaBench](https://huggingface.co/datasets/dwzhu/PaperBananaBench) ([PaperBanana](https://github.com/dwzhu-pku/PaperBanana)).
