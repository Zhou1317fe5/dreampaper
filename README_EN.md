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
  <img alt="Tauri 2" src="https://img.shields.io/badge/Tauri-2-24C8D5?logo=tauri&logoColor=white">
  <img alt="Rust stable" src="https://img.shields.io/badge/Rust-stable-000000?logo=rust&logoColor=white">
  <a href="LICENSE"><img alt="License PolyForm Noncommercial 1.0.0" src="https://img.shields.io/badge/License-PolyForm%20Noncommercial%201.0.0-F5A623"></a>
</p>

<p align="center">中文版：<a href="README.md">README.md</a></p>

---

## Why dreampaper

- **Template-driven** — figures use PaperBananaBench few-shot refs; slides lock layout and palette from your master image
- **Two-stage design** — structure / master first, then content, less template-copy and style drift
- **Bring your own models** — separate Design / Implement / Search profiles (OpenAI, Anthropic, image2, banana2, …)
- **Stage hooks** — event hooks stream the design log; context hooks inject contracts, inventories and evidence into each stage prompt as pluggable sections
- **Case memory + advisor** — design products are kept as cases, recalled by CJK-bigram FTS5 similarity, and an advisor role compares them into layout advice; rate a task good / fair / poor to steer later recalls
- **Slide visual grounding** — detect products and instruments in material, search appearance cues, draw real objects instead of labeled boxes
- **Fully local** — config and outputs under `~/.dreampaper/`; keys never enter the repo

---

## What's new

**Unreleased**
- Stage hooks: context injection is now an explicit, pluggable hook chain; the progress log names what each stage was injected with
- Case memory: design products are stored and recalled through CJK-bigram FTS5, up to 3 same-mode matches per task
- Advisor role: compares the recalled cases and injects reusable layouts, term mappings and failure modes to avoid
- Task rating: tag a finished task good / fair / poor; the rating lives in the case record and informs the advisor
- Multi-task tabs: up to 8 isolated task pages each on the figure and slide screens
- Result card: the workbench and download buttons are equal height and flush with the card edges

**v0.1.3**
- Reworked history manager: browse every past job, rerun any of them in one click
- Transparent generation log: the design model's analysis for each step streams alongside the job progress
- Automatic update checks and light/dark theme switching
- Assorted bug fixes and smoothness work

| History | Design log |
| --- | --- |
| ![history](examples/desktop/history.jpg) | ![log](examples/desktop/log.jpg) |

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

## Desktop downloads

Prefer not to set up Python? Grab the [latest release](../../releases/latest). The desktop build embeds a Rust backend, so there is no separate server to start.

| File | Platform |
| --- | --- |
| `dreampaper-*-setup.exe` | Windows installer (creates a desktop shortcut) |
| `dreampaper-*-portable.exe` | Windows portable (no install) |
| `dreampaper-*-x64-mac.dmg` | macOS Intel |
| `dreampaper-*-arm64-mac.dmg` | macOS Apple Silicon |

### First launch

The bundles are **not code-signed** (no developer certificate purchased), so the OS will block them once:

- **macOS**: double-clicking reports an unverified developer. Right-click the app → Open → Open again. One time only.
- **Windows**: SmartScreen shows "Windows protected your PC". Click "More info" → "Run anyway".
- The **Windows portable** build needs the WebView2 runtime. Windows 11 and Windows 10 21H2+ ship with it; on older systems install the
  [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) first, or use the installer (which handles it).

The desktop build stores config and outputs in the OS app-data directory rather than `~/.dreampaper/`:

| Platform | Path |
| --- | --- |
| macOS | `~/Library/Application Support/com.dreampaper.app/` |
| Windows | `%APPDATA%\com.dreampaper.app\` |

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

**The desktop build** does not read the repo directory. Import from the Templates page instead:

- **Import pack**: extract PaperBananaBench, click "Choose directory…", and select the extracted directory (the one containing `diagram/` and `plot/`) to import everything at once
- **Import image**: add your own templates one at a time with kind / category / description

Imported templates are copied into the app data directory, so switching branches or deleting the repo does not affect them.

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

## Model configuration

Three roles are configured separately, each with its own protocol / URL / model / key. Saving on the Settings page writes them to `~/.dreampaper/config.json`.

1. **`design model` (planner, needs multimodal input)** — reads the uploaded material and images in depth, arranges the content and layout of the target image, and produces the final drawing description. Strong style consistency comes from the template image. The upstream model must accept image input; OpenAI and Anthropic protocols are supported.
2. **`implement model` (renderer)** — takes the `design model` output and produces the final image. Supports OpenAI `v1/images/generations` and the gemini protocol; works with gpt-image-2 and nano-banana-2.
3. **`search model`** — extracts real-world objects from the uploaded material and images: hardware (radar, cameras, drones, …) and software products (Claude Code, Codex, Pi, …). It reuses real reference images from the web to guide the `design model`, suppressing invented visuals and enriching the result. Point it at xAI search (a Grok search model) or Tavily; DuckDuckGo is the default.

### Protocol dropdown

| Role | Protocol | Endpoint | Typical setup |
| --- | --- | --- | --- |
| design | `openai_responses` (default) | `{URL}/v1/responses` | `https://api.openai.com` + `gpt-5.4` |
| design | `openai_chat` | `{URL}/v1/chat/completions` | any OpenAI-compatible gateway |
| design | `anthropic_messages` | `{URL}/v1/messages` | `https://api.anthropic.com` |
| implement | `image2` (default) | `{URL}/v1/images/generations`, or `/v1/images/edits` when reference images are attached | `https://api.openai.com` + `gpt-image-2` |
| implement | `banana2` | `{URL}/{version}/interactions` | gemini-protocol gateway + `nano-banana-2` |
| search | `duckduckgo_html` (default) | DuckDuckGo HTML | no key needed; the key field is disabled |
| search | `tavily` | `{URL}/search` | `https://api.tavily.com`, API key required |
| search | `openai_chat (search model)` | `{URL}/chat/completions` | `https://api.x.ai/v1` + `grok-3` |

> The URL only needs the host; `/v1` is appended automatically when missing. `banana2` is the exception — its version comes from the Version field (default `v1beta`).

### Image parameter dropdowns

`implement` with `image2`:

| Field | Options | Default |
| --- | --- | --- |
| Size `size` | `auto` / `16:9` / `1024x1024` / `1200x675` / `928x1664` / `3000x1000` | `1200x675` |
| Quality `quality` | `auto` / `low` / `medium` / `high` / `hd` | `auto` |
| Format `output_format` | `png` / `jpeg` / `webp` | `png` |
| response `response_format` | `url` / `b64_json` | `url` |

`implement` with `banana2`:

| Field | Options | Default |
| --- | --- | --- |
| Ratio `aspect_ratio` | `16:9` / `4:3` / `1:1` / `3:2` | `16:9` |
| Sharpness `image_size` | `1K` / `2K` / `4K` | `4K` |
| Quality `thinking_level` | `minimal` / `high` | `high` |
| Format `mime_type` | `image/png` / `image/jpeg` / `image/webp` | `image/png` |
| Version `api_version` | free text | `v1beta` |

### Shared and runtime parameters

| Field | Role | Range | Default / suggestion |
| --- | --- | --- | --- |
| Timeout (s) | all | 5–1800 | design 120; implement 600–900 (sync image APIs are slow); search 15 |
| Retries | all | 0–8 | design 2 / implement 3 / search 1 |
| Stream | design | on / off | off |
| Results | search | 1–8 | 3 |
| Proxy | global | URL or port | `http://127.0.0.1:7890`; empty means no explicit proxy |
| Plan workers | global (slides) | 1–20 | empty = follow page count |
| Image workers | global (slides) | 1–20 | empty = follow page count; start at 1 to reduce gateway 502s |

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

---

## License

Licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE) — **noncommercial use is permitted, commercial use is not**.

| Use case | Permitted |
| --- | --- |
| Personal study, research, experiment, hobby projects | ✅ |
| Reading, modifying, redistributing the source | ✅ (must ship this license) |
| Internal production use at a company, paid or commercial services | ❌ |
| Selling this project or derivatives as a product | ❌ |
