# dreampaper

`dreampaper` 是一个本地运行的论文配图和 PPT 单页图片生成工具。首版使用 FastAPI + React，通过浏览器访问本机服务，支持 `paper_figure` 和 `ppt_slide` 两种工作流。

## 功能概览

- Paper figure：从 `PaperBananaBench` 手动选择 1-3 张 template 作为 few-shot 参考，输入图名和章节/方法描述后生成论文配图。
- PPT slide：上传一张单页 PPT template 图片，输入资料和页数后生成有序单页图片。
- 模型配置：design model 和 implement model 分开配置，支持自定义协议、`base_url`、`api_key` 和模型名。
- 本地持久化：模型配置保存到 `~/.dreampaper/config.json`，不会写入仓库文件。
- 输出控制：`image2` 使用尺寸、质量和输出格式；`banana2` 使用宽高比、清晰度、thinking level 和 mime type。

## 本地启动

准备 Python 环境并安装后端依赖：

```bash
python3 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```

安装前端依赖：

```bash
npm install
```

启动后端：

```bash
uvicorn backend.app.main:app --reload --host 127.0.0.1 --port 8000
```

启动前端：

```bash
npm run dev
```

打开 Vite 输出的本地地址，通常是 `http://127.0.0.1:5173`。

## 模型配置

进入页面左侧的“模型配置”，分别填写：

- Design model：选择 `openai_responses`、`openai_chat` 或 `anthropic_messages`，填写 `base_url`、模型名和 API key。
- Implement model：选择 `image2` 或 `banana2`，填写 `base_url`、模型名、API key 和输出控制项。

配置保存后写入 `~/.dreampaper/config.json`。前端再次读取配置时 API key 默认脱敏，只显示是否已配置和末尾提示。

## 基本工作流

### Paper figure

1. 选择 `Paper figure`。
2. 输入 figure title 和章节/方法描述。
3. 在 template gallery 中选择 1-3 张参考图。
4. 可调整宽高比、布局跟随和风格强度。
5. 点击生成后等待任务完成，在结果区查看图片。

### PPT slide

1. 选择 `PPT slide`。
2. 上传一张单页 PPT template 图片。
3. 输入资料/描述和目标页数。
4. 点击生成后，系统先分析母版元素，再按页码顺序生成图片。
5. 结果区按页面展示最终图片。

## 本地文件

- 配置文件：`~/.dreampaper/config.json`
- 上传素材：`~/.dreampaper/assets/`
- 任务记录和生成图片：`~/.dreampaper/jobs/`

这些文件都在用户本机目录中，不应提交到仓库。
