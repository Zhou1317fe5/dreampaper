from __future__ import annotations

import asyncio
import base64
import html
import json
import mimetypes
import re
import uuid
import zipfile
from collections.abc import Callable
from datetime import UTC, datetime
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, quote_plus, unquote, urlparse
from xml.etree import ElementTree

import httpx

from .adapters import DesignClient, ImplementClient, image_to_b64, parse_json_response, public_profile_snapshot, safe_error_message
from .assets import AssetStore
from .config import ConfigStore, app_home
from .models import JobCreateRequest, JobEvent, JobImage, JobRecord, PaperFigurePayload, PptSlidePayload
from .prompts import PromptStore, compose_prompt
from .templates import TemplateStore


IMAGE2_FALLBACK_OUTPUT = {
    "size": "1200x675",
    "quality": "auto",
    "output_format": "png",
    "response_format": "b64_json",
}
PPT_DESIGN_TIMEOUT_SECONDS = 300
MATERIAL_TEXT_LIMIT = 6000
PPT_PLAN_MATERIAL_LIMIT = 3200
PPT_PLAN_TEMPLATE_LIMIT = 2600
PPT_PLAN_VISUAL_CONTEXT_LIMIT = 1400
JSON_CONTEXT_RETRY_ATTEMPTS = 1
JSON_RETRY_EXCERPT_LIMIT = 4000


FORBIDDEN_TEMPLATE_COPY_PHRASES = (
    "copy the template",
    "duplicate the template",
    "trace the template",
    "use the template as a base image",
    "edit the template image",
    "background edit",
)

FIGURE_ASPECT_RATIOS = {"inherit", "16:9", "4:3", "1:1", "3:2", "2:3", "9:16"}
FIGURE_COMMON_FIELDS = (
    "visual_type",
    "layout_constraints",
    "semantic_constraints",
    "visual_constraints",
    "forbidden_errors",
    "quality_rubric",
    "visible_text",
    "implement_prompt",
)
DIAGRAM_SPEC_FIELDS = ("modules", "entities", "connections", "flow_direction", "grouping_hierarchy", "arrow_routing", "label_strategy")
PLOT_SPEC_FIELDS = (
    "chart_type",
    "data_fields",
    "axes",
    "units",
    "series_or_categories",
    "legend",
    "statistical_annotations",
    "data_integrity_rules",
)
MASTER_STYLE_FIELDS = (
    "canvas",
    "title_region",
    "safe_margins",
    "header_footer",
    "divider_lines",
    "palette",
    "typography",
    "module_style",
    "decorative_elements",
    "immutable_elements",
    "page_layout_rules",
    "forbidden_deviations",
)
PAGE_MASTER_BINDING_FIELDS = (
    "title_region",
    "safe_margins",
    "header_footer",
    "divider_lines",
    "palette",
    "typography",
    "module_style",
    "background",
)
MASTER_PROMPT_KEYWORDS = {
    "title_region": ("title region", "标题区"),
    "safe_margins": ("safe margin", "safe area", "安全边距", "安全区"),
    "header_footer": ("page number", "logo", "corner mark", "header", "footer", "页码", "角标", "页眉", "页脚"),
    "divider_lines": ("divider", "separator", "分割线", "分隔线"),
    "palette": ("palette", "color", "配色", "颜色"),
    "typography": ("typography", "font", "字体", "字号"),
    "module_style": ("module", "card", "border", "模块", "卡片", "边框"),
    "background": ("background", "背景"),
}
PAGE_VISUAL_PLAN_FIELDS = ("usage_decision", "elements", "text_visual_balance")
PAGE_VISUAL_ELEMENT_FIELDS = ("type", "subject", "source_reference", "placement", "style", "size_ratio")
PAGE_EMPHASIS_PLAN_FIELDS = ("keywords", "style_rules")
PAGE_EMPHASIS_KEYWORD_FIELDS = ("text", "style", "reason")
VISUAL_ASSET_SEARCH_TERM_LIMIT = 4
VISUAL_ASSET_SEARCH_RESULT_LIMIT = 3
VISUAL_ASSET_SEARCH_TIMEOUT_SECONDS = 8
VISUAL_ASSET_SEARCH_URL = "https://duckduckgo.com/html/"
VISUAL_ASSET_KNOWN_TERMS = (
    "PyTorch",
    "TensorFlow",
    "Docker",
    "Kubernetes",
    "GitHub",
    "GitLab",
    "OpenAI",
    "Claude",
    "Gemini",
    "Nano Banana",
    "WisArt",
    "MATLAB",
    "CUDA",
    "NVIDIA",
    "Raspberry Pi",
    "Arduino",
    "Figma",
    "Notion",
    "Slack",
    "Jira",
    "Confluence",
    "PostgreSQL",
    "MongoDB",
    "Redis",
    "MySQL",
    "AWS",
    "Azure",
    "GCP",
    "Hugging Face",
)
VISUAL_ASSET_TERM_STOPWORDS = {
    "A",
    "An",
    "And",
    "Body",
    "Card",
    "Create",
    "Data",
    "Figure",
    "Flow",
    "Input",
    "Material",
    "Model",
    "Output",
    "Page",
    "Prompt",
    "Result",
    "Slide",
    "Template",
    "The",
    "Use",
    "User",
}
SEARCH_RESULT_LINK_PATTERN = re.compile(r'<a[^>]+class="[^"]*result__a[^"]*"[^>]+href="([^"]+)"[^>]*>(.*?)</a>', re.IGNORECASE | re.DOTALL)
SEARCH_RESULT_SNIPPET_PATTERN = re.compile(r'<a[^>]+class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>|<div[^>]+class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</div>', re.IGNORECASE | re.DOTALL)
HTML_TAG_PATTERN = re.compile(r"<[^>]+>")
API_OUTPUT_PROMPT_PATTERN = re.compile(
    r"\b(?:size|quality|output_format|response_format|aspect_ratio|image_size|thinking_level|mime_type)\s*=\s*[^,.;\n]+[,.;]?\s*",
    re.IGNORECASE,
)
OUTPUT_SETTINGS_SENTENCE_PATTERN = re.compile(r"output settings preserved exactly:\s*[^.\n]*(?:\.|\n)?", re.IGNORECASE)


class DesignSchemaError(ValueError):
    """设计 JSON 已可解析但缺少必需结构化字段时抛出。"""


def now_iso() -> str:
    return datetime.now(UTC).isoformat()


class JobManager:
    def __init__(self, templates: TemplateStore, assets: AssetStore, prompts: PromptStore, config: ConfigStore) -> None:
        self.templates = templates
        self.assets = assets
        self.prompts = prompts
        self.config = config
        self.jobs: dict[str, JobRecord] = {}
        self.root = app_home() / "jobs"
        self.design = DesignClient()
        self.implement = ImplementClient()

    def create(self, request: JobCreateRequest) -> JobRecord:
        job_id = uuid.uuid4().hex
        timestamp = now_iso()
        record = JobRecord(
            id=job_id,
            mode=request.mode,
            status="queued",
            stage="queued",
            message="任务已排队",
            created_at=timestamp,
            updated_at=timestamp,
            events=[{"stage": "queued", "message": "任务已排队", "status": "running", "timestamp": timestamp}],
        )
        self.jobs[job_id] = record
        self._persist(record)
        return record

    def get(self, job_id: str) -> JobRecord:
        if job_id in self.jobs:
            record = self._normalize_record(self.jobs[job_id])
            self.jobs[job_id] = record
            return record
        path = self.root / job_id / "job.json"
        if path.exists():
            record = JobRecord.model_validate_json(path.read_text(encoding="utf-8"))
            record = self._normalize_record(record)
            self.jobs[job_id] = record
            self._persist(record)
            return record
        raise KeyError(job_id)

    async def run(self, job_id: str, request: JobCreateRequest) -> None:
        self._mark_stage(job_id, "started", "任务开始运行")
        try:
            if request.mode == "paper_figure":
                await self._run_paper(job_id, PaperFigurePayload.model_validate(request.payload))
            else:
                await self._run_ppt(job_id, PptSlidePayload.model_validate(request.payload))
        except Exception as exc:
            error_message = safe_error_message(exc)
            record = self.get(job_id)
            artifacts = dict(record.internal_artifacts)
            artifacts["error"] = {"message": error_message, "failed_at": now_iso()}
            self._mark_stage(job_id, "failed", error_message, status="failed", event_status="failed", internal_artifacts=artifacts)

    async def _run_paper(self, job_id: str, payload: PaperFigurePayload) -> None:
        self._mark_stage(job_id, "paper_validate", "校验 Figure 输入")
        if not payload.template_ids:
            raise ValueError("Paper figure requires at least one template")
        design_profile = self.config.active_profile("design")
        implement_profile = self.config.active_profile("implement")
        proxy_url = self.config.proxy_url()
        self._mark_stage(job_id, "paper_templates", "读取 template 和 few-shot 参考")
        selected = [self.templates.get(template_id) for template_id in payload.template_ids[:3]]
        selected_template_metadata = [self._template_metadata(item) for item in selected]
        template_images = [self._template_image_payload(item) for item in selected]
        template_summary = json.dumps(selected_template_metadata, ensure_ascii=False, indent=2)
        assets = [
            self.prompts.load("global/system.md"),
            self.prompts.load("global/figure_style.md"),
            self.prompts.load("modes/paper_figure/design.md"),
            self.prompts.load("modes/paper_figure/diagram_rules.md"),
            self.prompts.load("modes/paper_figure/plot_rules.md"),
            self.prompts.load("modes/paper_figure/validator.md"),
        ]
        user_context = {
            "Figure title": payload.figure_title.strip(),
            "Section description": payload.section_description.strip(),
            "Aspect ratio": payload.aspect_ratio,
            "Layout fidelity": payload.layout_fidelity,
            "Style strength": payload.style_strength,
            "Candidate count": payload.candidate_count,
            "Custom prompt": payload.custom_prompt or "None",
        }
        network_context = {"proxy_url": proxy_url}
        self._mark_stage(job_id, "paper_prompt", "拼接 design prompt")
        user_prompt, prompt_assets = compose_prompt(
            assets,
            {
                "Selected Template Metadata": template_summary,
                "User Input": json.dumps(user_context, ensure_ascii=False, indent=2),
                "Output Contract": self._paper_contract(),
            },
        )
        design_request = self._design_request_summary(design_profile, prompt_assets, user_prompt, template_images)
        self._merge_artifacts(
            job_id,
            {
                "normalized_input": user_context,
                "network": network_context,
                "selected_templates": selected_template_metadata,
                "prompt_assets": prompt_assets,
                "design_model_request": design_request,
            },
        )
        self._mark_stage(job_id, "paper_design", "调用 design model 生成制图方案")
        design_text = await self.design.generate(design_profile, assets[0]["content"], user_prompt, template_images, proxy_url=proxy_url)
        self._mark_stage(job_id, "paper_parse", "解析并校验 design JSON")
        design_json, _, paper_schema_retry = await self._parse_validate_or_fill_missing(
            design_profile,
            assets[0]["content"],
            user_prompt,
            design_text,
            template_images,
            self._validate_paper_design,
            timeout_seconds=None,
            proxy_url=proxy_url,
        )
        implement_prompt = self._paper_implement_prompt(design_json)
        self._mark_stage(job_id, "paper_implement", "调用 implement model 生成图片")
        image_b64 = await self.implement.generate(implement_profile, implement_prompt, proxy_url=proxy_url)
        self._mark_stage(job_id, "paper_save", "保存生成图片")
        image = self._save_image(job_id, "paper_figure.png", image_b64)
        self._mark_stage(job_id, "completed", "任务完成", status="succeeded", event_status="succeeded", images=[image])
        self._update(
            job_id,
            internal_artifacts={
                **self.get(job_id).internal_artifacts,
                "design_model_response": {"raw_text": design_text, "parsed_json": design_json},
                "design_response": design_json,
                "implement_model_request": self._implement_request_summary(implement_profile, implement_prompt, {}, 0),
                "implement_model_response": self._image_response_summary(image, image_b64),
                "implement_prompts": [implement_prompt],
                "retries": {"design_json_repair": "attempted only on parse failure", "paper_schema_fill": paper_schema_retry},
            },
        )

    async def _run_ppt(self, job_id: str, payload: PptSlidePayload) -> None:
        self._mark_stage(job_id, "ppt_validate", "校验 Slide 输入")
        app_config = self.config.load()
        design_profile = self.config.active_profile("design")
        implement_profile = self.config.active_profile("implement")
        page_plan_concurrency = self._resolve_ppt_concurrency(app_config.ppt_page_plan_concurrency, payload.page_count)
        image_concurrency = self._resolve_ppt_concurrency(app_config.ppt_image_concurrency, payload.page_count)
        proxy_url = self.config.proxy_url()
        ppt_design_timeout = max(design_profile.timeout_seconds, PPT_DESIGN_TIMEOUT_SECONDS)
        self._mark_stage(job_id, "ppt_template", "读取 template 图片")
        template_path, template_mime = self.assets.get(payload.template_asset_id)
        template_image = {"filename": template_path.name, "mime_type": template_mime, "b64": image_to_b64(template_path)}
        self._mark_stage(job_id, "ppt_material", "整理资料输入")
        material_assets = [self._material_asset_summary(asset_id) for asset_id in payload.material_asset_ids]
        material_context = self._compose_material_context(payload.material_text, material_assets)
        if not material_context.strip():
            raise ValueError("PPT slide requires material text or material files")
        self._mark_stage(job_id, "ppt_visual_assets", "检索产品/工具视觉素材线索")
        visual_asset_context = await self._build_visual_asset_context(material_context, proxy_url)
        visual_asset_prompt = self._visual_asset_context_text(visual_asset_context)
        ppt_output = self._ppt_output_defaults(implement_profile)
        normalized_input = {
            "template_asset_id": payload.template_asset_id,
            "material_text": payload.material_text.strip(),
            "material_asset_ids": payload.material_asset_ids,
            "material_assets": material_assets,
            "page_count": payload.page_count,
            "custom_prompt": payload.custom_prompt or "None",
            "output": ppt_output,
            "design_timeout_seconds": ppt_design_timeout,
            "visual_asset_search": {
                "enabled": True,
                "terms": visual_asset_context.get("terms", []),
                "degraded": visual_asset_context.get("degraded", False),
            },
        }
        network_context = {
            "proxy_url": proxy_url,
            "visual_asset_search": {
                "enabled": True,
                "term_limit": VISUAL_ASSET_SEARCH_TERM_LIMIT,
                "result_limit": VISUAL_ASSET_SEARCH_RESULT_LIMIT,
                "timeout_seconds": VISUAL_ASSET_SEARCH_TIMEOUT_SECONDS,
            },
        }
        analyzer_assets = [
            self.prompts.load("global/system.md"),
            self.prompts.load("modes/ppt_slide/analyzer.md"),
            self.prompts.load("modes/ppt_slide/master_rules.md"),
        ]
        analyzer_prompt, analyzer_prompt_assets = compose_prompt(
            analyzer_assets,
            {"Output Contract": self._template_analysis_contract()},
        )
        self._merge_artifacts(
            job_id,
            {
                "normalized_input": normalized_input,
                "network": network_context,
                "prompt_assets": analyzer_prompt_assets,
                "template_asset_id": payload.template_asset_id,
                "design_model_request": self._design_request_summary(
                    design_profile,
                    analyzer_prompt_assets,
                    analyzer_prompt,
                    [template_image],
                    timeout_seconds=ppt_design_timeout,
                ),
            },
        )
        self._mark_stage(job_id, "ppt_analyze", "调用 design model 分析 template")
        analysis_text = await self.design.generate(
            design_profile,
            analyzer_assets[0]["content"],
            analyzer_prompt,
            [template_image],
            timeout_seconds=ppt_design_timeout,
            proxy_url=proxy_url,
        )
        self._mark_stage(job_id, "ppt_parse_template", "解析 template 分析结果")
        template_analysis, _, template_schema_retry = await self._parse_validate_or_fill_missing(
            design_profile,
            analyzer_assets[0]["content"],
            analyzer_prompt,
            analysis_text,
            [template_image],
            self._validate_template_analysis,
            timeout_seconds=ppt_design_timeout,
            proxy_url=proxy_url,
        )
        page_assets = [
            self.prompts.load("global/system.md"),
            self.prompts.load("modes/ppt_slide/design.md"),
            self.prompts.load("styles/academic_ppt.md"),
        ]
        compact_template_analysis = self._compact_template_analysis(template_analysis)
        compact_material_context = self._truncate_text(material_context, PPT_PLAN_MATERIAL_LIMIT)
        compact_visual_asset_prompt = self._truncate_text(visual_asset_prompt, PPT_PLAN_VISUAL_CONTEXT_LIMIT)
        self._mark_stage(job_id, "ppt_outline_prompt", "拼接整套大纲规划 prompt")
        outline_prompt, outline_prompt_assets = compose_prompt(
            page_assets,
            {
                "Task Mode": "Deck outline mode. Plan only the deck narrative and lightweight page briefs. Do not write page-level implement_prompt.",
                "Template Analysis": compact_template_analysis,
                "Material": compact_material_context,
                "Visual Asset Search Context": compact_visual_asset_prompt,
                "Page Count": payload.page_count,
                "Custom Prompt": payload.custom_prompt or "None",
                "Output Contract": self._ppt_outline_contract(payload.page_count),
            },
        )
        all_prompt_assets = analyzer_prompt_assets + outline_prompt_assets
        self._merge_artifacts(
            job_id,
            {
                "template_analysis": template_analysis,
                "visual_asset_context": visual_asset_context,
                "page_planner_compaction": {
                    "template_chars": len(compact_template_analysis),
                    "material_chars": len(compact_material_context),
                    "visual_asset_chars": len(compact_visual_asset_prompt),
                    "outline_prompt_chars": len(outline_prompt),
                    "page_plan_concurrency": page_plan_concurrency,
                    "image_concurrency": image_concurrency,
                    "removed_redundant_master_rules_asset": True,
                },
                "prompt_assets": all_prompt_assets,
                "design_model_response": {"template_analysis_raw_text": analysis_text, "template_analysis_json": template_analysis},
                "outline_design_model_request": self._design_request_summary(
                    design_profile,
                    outline_prompt_assets,
                    outline_prompt,
                    [],
                    timeout_seconds=ppt_design_timeout,
                ),
            },
        )
        self._mark_stage(job_id, "ppt_outline", "调用 design model 规划整套大纲")
        outline_text = await self.design.generate(
            design_profile,
            page_assets[0]["content"],
            outline_prompt,
            [],
            timeout_seconds=ppt_design_timeout,
            proxy_url=proxy_url,
        )
        self._mark_stage(job_id, "ppt_parse_outline", "解析并校验整套大纲")
        outline_json, deck_outline, outline_schema_retry = await self._parse_validate_or_fill_missing(
            design_profile,
            page_assets[0]["content"],
            outline_prompt,
            outline_text,
            [],
            lambda parsed: self._validate_ppt_outline(parsed, payload.page_count),
            timeout_seconds=ppt_design_timeout,
            proxy_url=proxy_url,
        )
        page_briefs = deck_outline["page_briefs"]
        self._mark_stage(job_id, "ppt_page_plan_queue", f"按并发 {page_plan_concurrency} 排队规划 {payload.page_count} 页")
        page_results = await self._run_in_ordered_batches(
            page_briefs,
            page_plan_concurrency,
            lambda brief: self._plan_single_ppt_page(
                job_id=job_id,
                profile=design_profile,
                system_prompt=page_assets[0]["content"],
                page_assets=page_assets,
                template_analysis=template_analysis,
                compact_template_analysis=compact_template_analysis,
                compact_material_context=compact_material_context,
                compact_visual_asset_prompt=compact_visual_asset_prompt,
                deck_outline=deck_outline,
                page_brief=brief,
                custom_prompt=payload.custom_prompt or "None",
                timeout_seconds=ppt_design_timeout,
                proxy_url=proxy_url,
            ),
        )
        pages_json = {"pages": [result["page"] for result in page_results]}
        self._mark_stage(job_id, "ppt_merge_pages", "合并并校验页面规划")
        pages = self._validate_ppt_pages(pages_json, payload.page_count, template_analysis)
        pages = self._apply_ppt_master_prompt_prefix(pages, template_analysis)
        self._mark_stage(job_id, "ppt_implement_queue", f"按并发 {image_concurrency} 排队生成图片")
        implement_results = await self._run_in_ordered_batches(
            pages,
            image_concurrency,
            lambda page: self._implement_single_ppt_page(job_id, implement_profile, page, ppt_output, proxy_url),
        )
        images = [result["image"] for result in implement_results]
        implement_prompts = [result["prompt"] for result in implement_results]
        implement_requests = [result["request"] for result in implement_results]
        implement_responses = [result["response"] for result in implement_results]
        self._mark_stage(job_id, "completed", "任务完成", status="succeeded", event_status="succeeded", images=images)
        self._update(
            job_id,
            internal_artifacts={
                **self.get(job_id).internal_artifacts,
                "outline_design_model_response": {"raw_text": outline_text, "parsed_json": outline_json, "deck_outline": deck_outline},
                "page_design_model_response": {
                    "parsed_json": pages_json,
                    "workers": [
                        {
                            "page": result["page"]["page"],
                            "raw_text": result["raw_text"],
                            "parsed_json": result["parsed_json"],
                            "request": result["request"],
                        }
                        for result in page_results
                    ],
                },
                "page_plan": pages,
                "design_response": {"template_analysis": template_analysis, "deck_outline": deck_outline, "pages": pages},
                "implement_model_request": implement_requests,
                "implement_model_response": implement_responses,
                "implement_prompts": implement_prompts,
                "retries": {
                    "design_json_repair": "attempted only on parse failure",
                    "template_schema_fill": template_schema_retry,
                    "outline_schema_fill": outline_schema_retry,
                    "page_schema_fill": {str(result["page"]["page"]): result["retry"] for result in page_results},
                },
            },
        )

    @staticmethod
    def _resolve_ppt_concurrency(configured: int | None, page_count: int) -> int:
        default_concurrency = max(1, page_count)
        requested = configured if configured is not None else default_concurrency
        return min(max(1, requested), default_concurrency)

    async def _run_in_ordered_batches(self, items: list[Any], concurrency: int, worker: Callable[[Any], Any]) -> list[Any]:
        results: list[Any] = []
        limit = max(1, concurrency)
        for start in range(0, len(items), limit):
            batch = items[start : start + limit]
            results.extend(await asyncio.gather(*(worker(item) for item in batch)))
        return results

    async def _plan_single_ppt_page(
        self,
        *,
        job_id: str,
        profile,
        system_prompt: str,
        page_assets: list[dict[str, str]],
        template_analysis: dict[str, Any],
        compact_template_analysis: str,
        compact_material_context: str,
        compact_visual_asset_prompt: str,
        deck_outline: dict[str, Any],
        page_brief: dict[str, Any],
        custom_prompt: str,
        timeout_seconds: int,
        proxy_url: str | None,
    ) -> dict[str, Any]:
        page_number = int(page_brief.get("page") or 0)
        self._mark_stage(job_id, f"ppt_page_prompt_{page_number}", f"拼接第 {page_number} 页规划 prompt")
        page_prompt, page_prompt_assets = compose_prompt(
            page_assets,
            {
                "Task Mode": "Single-page worker mode. Return exactly one page object only. Do not plan or output other pages.",
                "Template Analysis": compact_template_analysis,
                "Deck Outline": json.dumps(deck_outline, ensure_ascii=False, separators=(",", ":")),
                "Current Page Brief": json.dumps(page_brief, ensure_ascii=False, indent=2),
                "Adjacent Page Context": json.dumps(self._adjacent_page_context(deck_outline, page_number), ensure_ascii=False, indent=2),
                "Material": compact_material_context,
                "Visual Asset Search Context": compact_visual_asset_prompt,
                "Custom Prompt": custom_prompt,
                "Output Contract": self._ppt_single_page_contract(page_number),
            },
        )
        request = self._design_request_summary(profile, page_prompt_assets, page_prompt, [], timeout_seconds=timeout_seconds)
        self._mark_stage(job_id, f"ppt_page_plan_{page_number}", f"规划第 {page_number} 页内容")
        page_text = await self.design.generate(
            profile,
            system_prompt,
            page_prompt,
            [],
            timeout_seconds=timeout_seconds,
            proxy_url=proxy_url,
        )
        page_json, page, retry = await self._parse_validate_or_fill_missing(
            profile,
            system_prompt,
            page_prompt,
            page_text,
            [],
            lambda parsed: self._validate_ppt_single_page(parsed, page_number, template_analysis),
            timeout_seconds=timeout_seconds,
            proxy_url=proxy_url,
        )
        return {"page": page, "raw_text": page_text, "parsed_json": page_json, "retry": retry, "request": request}

    async def _implement_single_ppt_page(
        self,
        job_id: str,
        profile,
        page: dict[str, Any],
        ppt_output: dict[str, Any],
        proxy_url: str | None,
    ) -> dict[str, Any]:
        page_number = int(page["page"])
        prompt = page["implement_prompt"]
        self._mark_stage(job_id, f"ppt_implement_{page_number}", f"生成第 {page_number} 页图片")
        image_b64 = await self.implement.generate(profile, prompt, output_overrides=ppt_output, proxy_url=proxy_url)
        image = self._save_image(job_id, f"slide_{page_number}.png", image_b64)
        return {
            "image": image,
            "prompt": prompt,
            "request": self._implement_request_summary(profile, prompt, ppt_output, 0, page=page_number),
            "response": self._image_response_summary(image, image_b64, page=page_number),
        }

    async def _parse_or_repair(
        self,
        profile,
        system_prompt: str,
        original_prompt: str,
        text: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None = None,
        proxy_url: str | None = None,
    ) -> dict[str, Any]:
        try:
            return parse_json_response(text)
        except Exception as first_error:
            last_error = first_error
            retry_text = ""
            for attempt in range(1, JSON_CONTEXT_RETRY_ATTEMPTS + 1):
                retry_prompt = (
                    f"{original_prompt}\n\n"
                    "The previous response for this exact task was not parseable JSON. "
                    "Regenerate the answer using the same task context and return strict JSON only. "
                    "Do not wrap in Markdown. Do not explain. Do not omit required fields. "
                    "Fix JSON syntax issues such as missing commas, dangling quotes, trailing prose, and unescaped newlines.\n\n"
                    f"Parse error: {safe_error_message(last_error)}\n"
                    f"Previous invalid output excerpt:\n{text[:JSON_RETRY_EXCERPT_LIMIT]}"
                )
                retry_text = await self.design.generate(
                    profile,
                    system_prompt,
                    retry_prompt,
                    images,
                    timeout_seconds=timeout_seconds,
                    proxy_url=proxy_url,
                )
                try:
                    return parse_json_response(retry_text)
                except Exception as retry_error:
                    last_error = retry_error
            repair_prompt = (
                "The previous model output was not valid JSON. Convert it into strict JSON only, preserving all useful content.\n"
                "Return JSON only. Do not wrap in Markdown. Do not explain.\n\n"
                f"Original task:\n{original_prompt}\n\nInvalid output:\n{(retry_text or text)[:JSON_RETRY_EXCERPT_LIMIT]}"
            )
            repaired = await self.design.generate(profile, system_prompt, repair_prompt, images, timeout_seconds=timeout_seconds, proxy_url=proxy_url)
            return parse_json_response(repaired)

    async def _parse_validate_or_fill_missing(
        self,
        profile,
        system_prompt: str,
        original_prompt: str,
        text: str,
        images: list[dict[str, str]],
        validator: Callable[[dict[str, Any]], Any],
        timeout_seconds: int | None = None,
        proxy_url: str | None = None,
    ) -> tuple[dict[str, Any], Any, dict[str, Any]]:
        parsed = await self._parse_or_repair(profile, system_prompt, original_prompt, text, images, timeout_seconds=timeout_seconds, proxy_url=proxy_url)
        try:
            return parsed, validator(parsed), {"attempted": False}
        except DesignSchemaError as exc:
            fill_prompt = (
                "The previous model output was valid JSON but failed the required structured output contract.\n"
                "Return strict JSON only. Preserve all valid content and do not redesign the figure, slide master, or page plan.\n"
                "Only fill, normalize, or add the missing required fields and constraints named by the validation error.\n\n"
                f"Validation error:\n{exc}\n\nOriginal task:\n{original_prompt}\n\nCurrent JSON:\n"
                f"{json.dumps(parsed, ensure_ascii=False, indent=2)}"
            )
            filled_text = await self.design.generate(profile, system_prompt, fill_prompt, images, timeout_seconds=timeout_seconds, proxy_url=proxy_url)
            filled = parse_json_response(filled_text)
            return filled, validator(filled), {"attempted": True, "reason": str(exc), "raw_text": filled_text[:2000]}

    def _template_image_payload(self, item: dict[str, Any]) -> dict[str, str]:
        path = self.templates._image_path(item)
        mime_type = mimetypes.guess_type(path.name)[0] or "image/jpeg"
        return {"filename": path.name, "mime_type": mime_type, "b64": image_to_b64(path)}

    def _template_metadata(self, item: dict[str, Any]) -> dict[str, Any]:
        additional = item.get("additional_info") or {}
        return {
            "id": item["template_id"],
            "kind": item["kind"],
            "category": item.get("category") or item.get("original_category"),
            "visual_intent": item.get("visual_intent"),
            "content": item.get("content"),
            "rounded_ratio": additional.get("rounded_ratio"),
            "path_to_gt_image": item.get("path_to_gt_image"),
        }

    def _material_asset_summary(self, asset_id: str) -> dict[str, Any]:
        metadata = self.assets.metadata(asset_id)
        path, mime_type = self.assets.get(asset_id)
        text, parser = self._extract_material_text(path, mime_type)
        excerpt = text[:MATERIAL_TEXT_LIMIT]
        return {
            "id": asset_id,
            "filename": metadata.get("filename") or path.name,
            "mime_type": mime_type,
            "bytes": path.stat().st_size if path.exists() else 0,
            "parser": parser,
            "text_excerpt": excerpt,
            "text_length": len(text),
            "truncated": len(text) > len(excerpt),
        }

    @staticmethod
    def _compose_material_context(material_text: str, material_assets: list[dict[str, Any]]) -> str:
        sections: list[str] = []
        if material_text.strip():
            sections.append(f"User text material:\n{material_text.strip()}")
        for asset in material_assets:
            header = f"File material: {asset['filename']} ({asset['mime_type']}, {asset['bytes']} bytes, parser={asset['parser']})"
            if asset.get("truncated"):
                header += f", excerpt={len(asset.get('text_excerpt') or '')}/{asset.get('text_length')} chars"
            excerpt = asset.get("text_excerpt") or "No extractable text available; use filename and file type as context only."
            sections.append(f"{header}\n{excerpt}")
        return "\n\n---\n\n".join(sections)

    async def _build_visual_asset_context(self, material_context: str, proxy_url: str | None = None) -> dict[str, Any]:
        terms = self._extract_visual_asset_terms(material_context)
        if not terms:
            return {
                "enabled": True,
                "degraded": True,
                "terms": [],
                "items": [],
                "message": "No explicit product/tool/platform terms were detected; use generic semantic icons or object illustrations only when content benefits.",
            }
        tasks = [self._search_visual_asset_term(term, proxy_url) for term in terms]
        results = await asyncio.gather(*tasks, return_exceptions=True)
        items: list[dict[str, Any]] = []
        errors: list[str] = []
        for term, result in zip(terms, results, strict=False):
            if isinstance(result, Exception):
                errors.append(f"{term}: {safe_error_message(result)}")
                items.append({"term": term, "query": self._visual_asset_search_query(term), "results": [], "error": safe_error_message(result)})
                continue
            items.append(result)
        has_sources = any(item.get("results") for item in items)
        return {
            "enabled": True,
            "degraded": not has_sources,
            "terms": terms,
            "items": items,
            "errors": errors,
            "message": "Use only text summaries and source URLs; no network image is downloaded, cached, or passed to the implement model.",
        }

    @classmethod
    def _extract_visual_asset_terms(cls, material_context: str) -> list[str]:
        text = material_context[:MATERIAL_TEXT_LIMIT]
        candidates: list[str] = []
        lowered = text.lower()
        for term in VISUAL_ASSET_KNOWN_TERMS:
            if term.lower() in lowered:
                candidates.append(term)
        patterns = [
            r"\b[A-Z][A-Za-z0-9.+#-]{1,}(?:\s+[A-Z0-9][A-Za-z0-9.+#-]{1,}){0,2}\b",
            r"\b[A-Z]{2,}(?:[-\s][A-Z0-9]{2,}){0,2}\b",
            r"\b[A-Za-z][A-Za-z0-9.+#-]{1,}\s*(?:平台|工具|框架|模型|软件|系统|设备)\b",
        ]
        for pattern in patterns:
            candidates.extend(match.group(0) for match in re.finditer(pattern, text))
        return cls._dedupe_visual_asset_terms(candidates)

    @staticmethod
    def _dedupe_visual_asset_terms(candidates: list[str]) -> list[str]:
        terms: list[str] = []
        seen: set[str] = set()
        for candidate in candidates:
            term = re.sub(r"\s+", " ", candidate).strip(" ,.;:()[]{}<>，。；：（）【】")
            term = re.sub(r"\s*(平台|工具|框架|模型|软件|系统|设备)$", "", term).strip()
            if len(term) < 2 or len(term) > 48:
                continue
            if term in VISUAL_ASSET_TERM_STOPWORDS:
                continue
            if term.lower() in {item.lower() for item in VISUAL_ASSET_TERM_STOPWORDS}:
                continue
            key = term.lower()
            if key in seen:
                continue
            seen.add(key)
            terms.append(term)
            if len(terms) >= VISUAL_ASSET_SEARCH_TERM_LIMIT:
                break
        return terms

    @staticmethod
    def _visual_asset_search_query(term: str) -> str:
        return f"{term} official logo product render visual appearance"

    async def _search_visual_asset_term(self, term: str, proxy_url: str | None = None) -> dict[str, Any]:
        query = self._visual_asset_search_query(term)
        timeout = httpx.Timeout(VISUAL_ASSET_SEARCH_TIMEOUT_SECONDS)
        headers = {"User-Agent": "Mozilla/5.0 dreampaper visual asset context"}
        async with httpx.AsyncClient(timeout=timeout, proxy=proxy_url, follow_redirects=True, headers=headers) as client:
            response = await client.get(f"{VISUAL_ASSET_SEARCH_URL}?q={quote_plus(query)}")
        if response.status_code >= 400:
            return {"term": term, "query": query, "results": [], "error": f"HTTP {response.status_code}"}
        return {"term": term, "query": query, "results": self._parse_visual_asset_search_results(response.text)}

    @classmethod
    def _parse_visual_asset_search_results(cls, html_text: str) -> list[dict[str, str]]:
        links = SEARCH_RESULT_LINK_PATTERN.findall(html_text)
        snippets = SEARCH_RESULT_SNIPPET_PATTERN.findall(html_text)
        cleaned_snippets = [cls._clean_search_html(first or second) for first, second in snippets]
        results: list[dict[str, str]] = []
        for index, (href, title_html) in enumerate(links[:VISUAL_ASSET_SEARCH_RESULT_LIMIT]):
            url = cls._normalize_search_result_url(html.unescape(href))
            if not url.startswith(("http://", "https://")):
                continue
            results.append(
                {
                    "title": cls._clean_search_html(title_html),
                    "url": url,
                    "snippet": cleaned_snippets[index] if index < len(cleaned_snippets) else "",
                }
            )
        return results

    @staticmethod
    def _normalize_search_result_url(url: str) -> str:
        parsed = urlparse(url)
        query = parse_qs(parsed.query)
        if "uddg" in query and query["uddg"]:
            return unquote(query["uddg"][0])
        return url

    @staticmethod
    def _clean_search_html(value: str) -> str:
        text = HTML_TAG_PATTERN.sub(" ", value)
        text = html.unescape(text)
        text = re.sub(r"\s+", " ", text).strip()
        return text[:260]

    @staticmethod
    def _visual_asset_context_text(context: dict[str, Any]) -> str:
        terms = context.get("terms") if isinstance(context.get("terms"), list) else []
        if not terms:
            return (
                "No specific product/tool/platform visual sources were identified from the material. "
                "Use generic semantic icons, equipment/object illustrations, or abstract logo-like symbols only when they help the content. "
                "Do not invent real brand logos."
            )
        lines = [
            "Runtime visual asset search context. Use this as text-only evidence for product/tool visuals; no images are downloaded or passed to the implement model.",
            "Prefer official or reliable sources. If no reliable source is listed for a term, use a generic semantic icon or illustrative object instead of inventing a real brand logo.",
        ]
        for item in context.get("items", []):
            term = item.get("term")
            query = item.get("query")
            lines.append(f"- Term: {term}; query: {query}")
            results = item.get("results") if isinstance(item.get("results"), list) else []
            if not results:
                reason = item.get("error") or "no reliable result"
                lines.append(f"  Source status: {reason}. Use generic semantic visual elements only.")
                continue
            for result in results:
                lines.append(f"  Source: {result.get('title')} | {result.get('url')} | {result.get('snippet')}")
        return "\n".join(lines)

    @staticmethod
    def _truncate_text(text: str, limit: int) -> str:
        if len(text) <= limit:
            return text
        return f"{text[:limit]}\n\n[Truncated from {len(text)} chars to {limit} chars for page planning latency.]"

    @classmethod
    def _compact_template_analysis(cls, template_analysis: dict[str, Any]) -> str:
        wrapper = template_analysis.get("template_analysis") if isinstance(template_analysis.get("template_analysis"), dict) else template_analysis
        if not isinstance(wrapper, dict):
            return cls._truncate_text(json.dumps(template_analysis, ensure_ascii=False), PPT_PLAN_TEMPLATE_LIMIT)
        master = wrapper.get("master_style_spec") if isinstance(wrapper.get("master_style_spec"), dict) else {}
        compact = {
            "master_style_summary": wrapper.get("master_style_summary"),
            "master_style_spec": {
                "canvas": master.get("canvas"),
                "title_region": master.get("title_region"),
                "safe_margins": master.get("safe_margins"),
                "header_footer": master.get("header_footer"),
                "divider_lines": master.get("divider_lines"),
                "palette": master.get("palette"),
                "typography": master.get("typography"),
                "module_style": master.get("module_style"),
                "decorative_elements": master.get("decorative_elements"),
                "immutable_elements": master.get("immutable_elements"),
                "forbidden_deviations": master.get("forbidden_deviations"),
            },
            "global_constraints": wrapper.get("global_constraints"),
            "immutable_elements": wrapper.get("immutable_elements"),
            "page_layout_rules": wrapper.get("page_layout_rules"),
        }
        return cls._truncate_text(json.dumps(compact, ensure_ascii=False, separators=(",", ":")), PPT_PLAN_TEMPLATE_LIMIT)

    @staticmethod
    def _extract_material_text(path: Path, mime_type: str) -> tuple[str, str]:
        suffix = path.suffix.lower()
        if suffix in {".txt", ".md", ".markdown", ".csv", ".tsv", ".json"} or mime_type.startswith("text/"):
            return path.read_text(encoding="utf-8", errors="ignore"), "text"
        if suffix == ".docx":
            try:
                with zipfile.ZipFile(path) as docx:
                    xml = docx.read("word/document.xml")
                root = ElementTree.fromstring(xml)
                text = "\n".join(node.text or "" for node in root.iter() if node.tag.endswith("}t"))
                return text, "docx-xml"
            except Exception as exc:
                return f"DOCX text extraction failed: {safe_error_message(exc)}", "docx-error"
        if suffix == ".pdf" or mime_type == "application/pdf":
            try:
                from pypdf import PdfReader  # type: ignore

                reader = PdfReader(str(path))
                text = "\n".join(page.extract_text() or "" for page in reader.pages[:20])
                return text, "pypdf"
            except Exception as exc:
                return f"PDF text extraction unavailable: {safe_error_message(exc)}", "pdf-unavailable"
        return "", "metadata-only"

    def _save_image(self, job_id: str, name: str, image_b64: str) -> JobImage:
        job_dir = self.root / job_id
        job_dir.mkdir(parents=True, exist_ok=True)
        path = job_dir / name
        path.write_bytes(base64.b64decode(image_b64))
        return JobImage(name=name, url=f"/api/jobs/{job_id}/images/{name}")

    def image_path(self, job_id: str, name: str) -> Path:
        path = self.root / job_id / name
        if not path.exists():
            raise FileNotFoundError(name)
        return path

    def _merge_artifacts(self, job_id: str, changes: dict[str, Any]) -> None:
        record = self.get(job_id)
        self._update(job_id, internal_artifacts={**record.internal_artifacts, **changes})

    def _mark_stage(
        self,
        job_id: str,
        stage: str,
        message: str,
        status: str = "running",
        event_status: str = "running",
        **changes: Any,
    ) -> None:
        record = self.get(job_id)
        event = JobEvent(stage=stage, message=message, status=event_status, timestamp=now_iso())
        self._update(job_id, status=status, stage=stage, message=message, events=[*record.events, event], **changes)

    @staticmethod
    def _normalize_record(record: JobRecord) -> JobRecord:
        message = (record.message or "").strip()
        artifacts = record.internal_artifacts or {}
        error = artifacts.get("error") if isinstance(artifacts.get("error"), dict) else {}
        error_message = str(error.get("message") or "").strip()
        timestamp = str(error.get("failed_at") or record.updated_at)

        if record.status == "failed":
            fallback = "任务失败，旧记录没有保存具体错误；请重新提交以获取阶段日志。"
            normalized_message = message or error_message or fallback
            events = record.events or [JobEvent(stage="failed", message=normalized_message, status="failed", timestamp=timestamp)]
            return record.model_copy(update={"message": normalized_message, "stage": record.stage if record.stage != "queued" else "failed", "events": events})

        if record.status == "succeeded" and not record.events:
            event = JobEvent(stage="completed", message=message or "任务完成", status="succeeded", timestamp=record.updated_at)
            return record.model_copy(update={"message": message or "任务完成", "stage": "completed", "events": [event]})

        if not record.events:
            normalized_message = message or ("任务已排队" if record.status == "queued" else "任务运行中")
            event = JobEvent(stage=record.stage or record.status, message=normalized_message, status="running", timestamp=record.updated_at)
            return record.model_copy(update={"message": normalized_message, "events": [event]})

        if not message:
            return record.model_copy(update={"message": record.events[-1].message})
        return record

    def _update(self, job_id: str, **changes) -> None:
        record = self.get(job_id)
        next_record = record.model_copy(update={"updated_at": now_iso(), **changes})
        self.jobs[job_id] = next_record
        self._persist(next_record)

    def _persist(self, record: JobRecord) -> None:
        job_dir = self.root / record.id
        job_dir.mkdir(parents=True, exist_ok=True)
        (job_dir / "job.json").write_text(record.model_dump_json(indent=2), encoding="utf-8")

    @staticmethod
    def _paper_implement_prompt(design_json: dict[str, Any]) -> str:
        figure = design_json.get("figure") if isinstance(design_json.get("figure"), dict) else design_json
        if not isinstance(figure, dict):
            raise ValueError("Design model response must be a JSON object")
        prompt = figure.get("implement_prompt")
        if not isinstance(prompt, str):
            raise DesignSchemaError("Design model response missing implement_prompt")
        return prompt

    @staticmethod
    def _is_filled(value: Any) -> bool:
        if value is None:
            return False
        if isinstance(value, str):
            return bool(value.strip())
        if isinstance(value, (list, dict, tuple, set)):
            return bool(value)
        return True

    @classmethod
    def _require_fields(cls, data: dict[str, Any], fields: tuple[str, ...], label: str) -> None:
        missing = [field for field in fields if not cls._is_filled(data.get(field))]
        if missing:
            raise DesignSchemaError(f"{label} missing required fields: {', '.join(missing)}")

    @staticmethod
    def _keyword_groups_present(text: str, groups: dict[str, tuple[str, ...]]) -> set[str]:
        lowered = text.lower()
        return {name for name, keywords in groups.items() if any(keyword.lower() in lowered for keyword in keywords)}

    @classmethod
    def _validate_no_copy_request(cls, prompt: str) -> None:
        lowered_prompt = prompt.lower()
        negations = ("do not ", "don't ", "never ", "no ", "not ", "avoid ", "must not ", "cannot ", "禁止", "不要", "不得", "不能", "不可")
        for phrase in FORBIDDEN_TEMPLATE_COPY_PHRASES:
            start = lowered_prompt.find(phrase)
            while start != -1:
                prefix = lowered_prompt[max(0, start - 24) : start]
                if not any(negation in prefix for negation in negations):
                    raise ValueError("Implement prompt requests direct template copying or editing")
                start = lowered_prompt.find(phrase, start + len(phrase))

    @classmethod
    def _validate_paper_design(cls, design_json: dict[str, Any]) -> None:
        figure = design_json.get("figure") if isinstance(design_json.get("figure"), dict) else design_json
        if not isinstance(figure, dict):
            raise DesignSchemaError("Design model response must contain a figure object")
        cls._require_fields(figure, FIGURE_COMMON_FIELDS, "Paper figure")
        implement_prompt = cls._paper_implement_prompt(design_json)
        if len(implement_prompt.strip()) < 220:
            raise DesignSchemaError("Paper figure implement_prompt is too short for the strengthened contract")
        aspect_ratio = figure.get("aspect_ratio")
        if aspect_ratio and aspect_ratio not in FIGURE_ASPECT_RATIOS:
            raise ValueError(f"Invalid paper figure aspect_ratio: {aspect_ratio}")
        visible_text = figure.get("visible_text", [])
        if not isinstance(visible_text, list) or any(not isinstance(item, str) or len(item.strip()) > 80 for item in visible_text):
            raise ValueError("Paper figure visible_text must be short label strings")
        cls._validate_no_copy_request(implement_prompt)
        prompt_groups = cls._keyword_groups_present(
            implement_prompt,
            {
                "publication": ("publication", "academic", "paper", "论文", "出版"),
                "faithfulness": ("faithful", "faithfulness", "grounded", "no hallucination", "忠实", "不虚构"),
                "conciseness": ("concise", "abstraction", "short label", "简洁", "抽象"),
                "readability": ("readable", "legible", "contrast", "可读", "对比"),
                "template_boundary": ("template", "reference", "not copy", "参考", "模板"),
            },
        )
        if len(prompt_groups) < 4:
            raise DesignSchemaError("Paper figure implement_prompt must cover publication quality, faithfulness, conciseness, readability, and template boundary")
        visual_type = str(figure.get("visual_type") or "").strip().lower()
        if visual_type in {"diagram", "workflow", "comparison", "mechanism"}:
            diagram_spec = figure.get("diagram_spec")
            if not isinstance(diagram_spec, dict):
                raise DesignSchemaError("Diagram figure missing diagram_spec")
            cls._require_fields(diagram_spec, DIAGRAM_SPEC_FIELDS, "Diagram spec")
            connections = diagram_spec.get("connections")
            if not isinstance(connections, list) or not connections:
                raise DesignSchemaError("Diagram spec connections must be a non-empty list")
            for index, connection in enumerate(connections, start=1):
                if not isinstance(connection, dict):
                    raise DesignSchemaError(f"Diagram connection {index} must be an object")
                cls._require_fields(connection, ("source", "target", "meaning"), f"Diagram connection {index}")
            return
        if visual_type in {"plot", "chart"}:
            plot_spec = figure.get("plot_spec")
            if not isinstance(plot_spec, dict):
                raise DesignSchemaError("Plot/chart figure missing plot_spec")
            cls._require_fields(plot_spec, PLOT_SPEC_FIELDS, "Plot spec")
            axes = plot_spec.get("axes")
            if not isinstance(axes, dict) or not cls._is_filled(axes.get("x")) or not cls._is_filled(axes.get("y")):
                raise DesignSchemaError("Plot spec axes must include x and y definitions")
            integrity = str(plot_spec.get("data_integrity_rules") or "")
            if len(integrity) < 40:
                raise DesignSchemaError("Plot spec data_integrity_rules must explicitly describe anti-distortion constraints")
            return
        raise DesignSchemaError("Paper figure visual_type must be diagram, workflow, comparison, mechanism, plot, or chart")

    @classmethod
    def _master_style_spec(cls, template_analysis: dict[str, Any]) -> dict[str, Any]:
        wrapper = template_analysis.get("template_analysis") if isinstance(template_analysis.get("template_analysis"), dict) else template_analysis
        if not isinstance(wrapper, dict):
            raise DesignSchemaError("Template analysis must be a JSON object")
        master = wrapper.get("master_style_spec")
        if not isinstance(master, dict):
            raise DesignSchemaError("Template analysis missing master_style_spec")
        return master

    @classmethod
    def _validate_template_analysis(cls, template_analysis: dict[str, Any]) -> None:
        wrapper = template_analysis.get("template_analysis") if isinstance(template_analysis.get("template_analysis"), dict) else template_analysis
        if not isinstance(wrapper, dict):
            raise DesignSchemaError("Template analysis must contain template_analysis object")
        master = cls._master_style_spec(template_analysis)
        cls._require_fields(master, MASTER_STYLE_FIELDS, "PPT master_style_spec")
        cls._require_fields(wrapper, ("global_constraints", "immutable_elements", "page_layout_rules"), "PPT template_analysis")
        immutable = wrapper.get("immutable_elements")
        if not isinstance(immutable, list) or len(immutable) < 3:
            raise DesignSchemaError("PPT immutable_elements must list at least title/page marker/divider or equivalent master elements")

    @classmethod
    def _validate_ppt_outline(cls, outline_json: dict[str, Any], page_count: int) -> dict[str, Any]:
        outline = outline_json.get("deck_outline") if isinstance(outline_json.get("deck_outline"), dict) else outline_json
        if not isinstance(outline, dict):
            raise DesignSchemaError("PPT outline response must be a JSON object")
        cls._require_fields(
            outline,
            ("deck_title", "deck_goal", "narrative_arc", "shared_prompt", "page_briefs"),
            "PPT deck_outline",
        )
        page_briefs = outline.get("page_briefs")
        if not isinstance(page_briefs, list):
            raise DesignSchemaError("PPT deck_outline.page_briefs must be a list")
        if len(page_briefs) != page_count:
            raise ValueError(f"Deck outline returned {len(page_briefs)} page briefs, expected {page_count}")
        sorted_briefs = sorted(page_briefs, key=lambda item: item.get("page", 0) if isinstance(item, dict) else 0)
        actual_pages = [item.get("page") for item in sorted_briefs if isinstance(item, dict)]
        expected_pages = list(range(1, page_count + 1))
        if actual_pages != expected_pages:
            raise ValueError(f"PPT outline page briefs must be ordered 1..{page_count}; got {actual_pages}")
        for brief in sorted_briefs:
            if not isinstance(brief, dict):
                raise DesignSchemaError("Each PPT page brief must be an object")
            cls._require_fields(
                brief,
                (
                    "page",
                    "title",
                    "role",
                    "main_message",
                    "content_points",
                    "suggested_template",
                    "visual_direction",
                    "transition_from_previous",
                    "transition_to_next",
                ),
                f"Page brief {brief.get('page')}",
            )
            points = brief.get("content_points")
            if not isinstance(points, list) or not points:
                raise DesignSchemaError(f"Page brief {brief.get('page')} content_points must be a non-empty list")
        return {**outline, "page_briefs": sorted_briefs}

    @staticmethod
    def _adjacent_page_context(deck_outline: dict[str, Any], page_number: int) -> dict[str, Any]:
        briefs = deck_outline.get("page_briefs") if isinstance(deck_outline.get("page_briefs"), list) else []
        previous_brief = next((brief for brief in briefs if isinstance(brief, dict) and brief.get("page") == page_number - 1), None)
        next_brief = next((brief for brief in briefs if isinstance(brief, dict) and brief.get("page") == page_number + 1), None)
        return {"previous": previous_brief, "next": next_brief}

    @classmethod
    def _validate_ppt_single_page(
        cls,
        page_json: dict[str, Any],
        expected_page: int,
        template_analysis: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        if template_analysis is not None:
            cls._validate_template_analysis(template_analysis)
        page = page_json.get("page") if isinstance(page_json.get("page"), dict) else None
        if page is None:
            pages = page_json.get("pages")
            if isinstance(pages, list) and len(pages) == 1 and isinstance(pages[0], dict):
                page = pages[0]
        if not isinstance(page, dict):
            raise DesignSchemaError("Single-page worker response must contain one page object")
        if page.get("page") != expected_page:
            raise ValueError(f"Single-page worker returned page {page.get('page')}, expected {expected_page}")
        cls._validate_ppt_page_fields(page)
        return page

    @classmethod
    def _validate_ppt_page_fields(cls, page: dict[str, Any]) -> None:
        if not isinstance(page, dict):
            raise DesignSchemaError("Each PPT page must be an object")
        cls._require_fields(
            page,
            (
                "page",
                "selected_template",
                "title",
                "slide_type",
                "body_layout_plan",
                "master_style_binding",
                "visual_element_plan",
                "emphasis_plan",
                "visible_text",
                "implement_prompt",
            ),
            f"Page {page.get('page')}",
        )
        binding = page.get("master_style_binding")
        if not isinstance(binding, dict):
            raise DesignSchemaError(f"Page {page.get('page')} missing master_style_binding object")
        cls._require_fields(binding, PAGE_MASTER_BINDING_FIELDS, f"Page {page.get('page')} master_style_binding")
        visual_plan = page.get("visual_element_plan")
        if not isinstance(visual_plan, dict):
            raise DesignSchemaError(f"Page {page.get('page')} missing visual_element_plan object")
        cls._require_fields(visual_plan, PAGE_VISUAL_PLAN_FIELDS, f"Page {page.get('page')} visual_element_plan")
        visual_elements = visual_plan.get("elements")
        if not isinstance(visual_elements, list):
            raise DesignSchemaError(f"Page {page.get('page')} visual_element_plan.elements must be a list")
        if not visual_elements and "none" not in str(visual_plan.get("usage_decision") or "").lower() and "不用" not in str(visual_plan.get("usage_decision") or ""):
            raise DesignSchemaError(f"Page {page.get('page')} visual_element_plan must list elements or explicitly justify using none")
        for index, element in enumerate(visual_elements, start=1):
            if not isinstance(element, dict):
                raise DesignSchemaError(f"Page {page.get('page')} visual element {index} must be an object")
            cls._require_fields(element, PAGE_VISUAL_ELEMENT_FIELDS, f"Page {page.get('page')} visual element {index}")
        emphasis_plan = page.get("emphasis_plan")
        if not isinstance(emphasis_plan, dict):
            raise DesignSchemaError(f"Page {page.get('page')} missing emphasis_plan object")
        cls._require_fields(emphasis_plan, PAGE_EMPHASIS_PLAN_FIELDS, f"Page {page.get('page')} emphasis_plan")
        keywords = emphasis_plan.get("keywords")
        if not isinstance(keywords, list):
            raise DesignSchemaError(f"Page {page.get('page')} emphasis_plan.keywords must be a list")
        if len(keywords) > 5:
            raise DesignSchemaError(f"Page {page.get('page')} emphasis_plan may highlight at most 5 key phrases")
        for index, keyword in enumerate(keywords, start=1):
            if not isinstance(keyword, dict):
                raise DesignSchemaError(f"Page {page.get('page')} emphasis keyword {index} must be an object")
            cls._require_fields(keyword, PAGE_EMPHASIS_KEYWORD_FIELDS, f"Page {page.get('page')} emphasis keyword {index}")
            text = str(keyword.get("text") or "").strip()
            if len(text) > 24:
                raise DesignSchemaError(f"Page {page.get('page')} emphasis keyword {index} must be a short phrase")
        visible_text = page.get("visible_text")
        if not isinstance(visible_text, list) or any(not isinstance(item, str) or len(item.strip()) > 120 for item in visible_text):
            raise DesignSchemaError(f"Page {page.get('page')} visible_text must be short strings")
        prompt = page.get("implement_prompt")
        if not isinstance(prompt, str) or len(prompt.strip()) < 80:
            raise DesignSchemaError(f"Page {page.get('page')} missing usable page-specific implement_prompt")
        if API_OUTPUT_PROMPT_PATTERN.search(prompt) or OUTPUT_SETTINGS_SENTENCE_PATTERN.search(prompt):
            raise DesignSchemaError(f"Page {page.get('page')} implement_prompt must not include API output settings")
        cls._validate_no_copy_request(prompt)

    @classmethod
    def _validate_ppt_pages(
        cls,
        pages_json: dict[str, Any],
        page_count: int,
        template_analysis: dict[str, Any] | None = None,
    ) -> list[dict[str, Any]]:
        pages = pages_json.get("pages") or []
        if not isinstance(pages, list):
            raise DesignSchemaError("PPT design response pages must be a list")
        if len(pages) != page_count:
            raise ValueError(f"Design model returned {len(pages)} pages, expected {page_count}")
        sorted_pages = sorted(pages, key=lambda item: item.get("page", 0) if isinstance(item, dict) else 0)
        expected_pages = list(range(1, page_count + 1))
        actual_pages = [item.get("page") for item in sorted_pages if isinstance(item, dict)]
        if actual_pages != expected_pages:
            raise ValueError(f"PPT pages must be ordered 1..{page_count}; got {actual_pages}")
        if template_analysis is not None:
            cls._validate_template_analysis(template_analysis)
        for page in sorted_pages:
            if not isinstance(page, dict):
                raise DesignSchemaError("Each PPT page must be an object")
            cls._require_fields(
                page,
                (
                    "page",
                    "selected_template",
                    "title",
                    "slide_type",
                    "body_layout_plan",
                    "master_style_binding",
                    "visual_element_plan",
                    "emphasis_plan",
                    "visible_text",
                    "implement_prompt",
                ),
                f"Page {page.get('page')}",
            )
            binding = page.get("master_style_binding")
            if not isinstance(binding, dict):
                raise DesignSchemaError(f"Page {page.get('page')} missing master_style_binding object")
            cls._require_fields(binding, PAGE_MASTER_BINDING_FIELDS, f"Page {page.get('page')} master_style_binding")
            visual_plan = page.get("visual_element_plan")
            if not isinstance(visual_plan, dict):
                raise DesignSchemaError(f"Page {page.get('page')} missing visual_element_plan object")
            cls._require_fields(visual_plan, PAGE_VISUAL_PLAN_FIELDS, f"Page {page.get('page')} visual_element_plan")
            visual_elements = visual_plan.get("elements")
            if not isinstance(visual_elements, list):
                raise DesignSchemaError(f"Page {page.get('page')} visual_element_plan.elements must be a list")
            if not visual_elements and "none" not in str(visual_plan.get("usage_decision") or "").lower() and "不用" not in str(visual_plan.get("usage_decision") or ""):
                raise DesignSchemaError(f"Page {page.get('page')} visual_element_plan must list elements or explicitly justify using none")
            for index, element in enumerate(visual_elements, start=1):
                if not isinstance(element, dict):
                    raise DesignSchemaError(f"Page {page.get('page')} visual element {index} must be an object")
                cls._require_fields(element, PAGE_VISUAL_ELEMENT_FIELDS, f"Page {page.get('page')} visual element {index}")
            emphasis_plan = page.get("emphasis_plan")
            if not isinstance(emphasis_plan, dict):
                raise DesignSchemaError(f"Page {page.get('page')} missing emphasis_plan object")
            cls._require_fields(emphasis_plan, PAGE_EMPHASIS_PLAN_FIELDS, f"Page {page.get('page')} emphasis_plan")
            keywords = emphasis_plan.get("keywords")
            if not isinstance(keywords, list):
                raise DesignSchemaError(f"Page {page.get('page')} emphasis_plan.keywords must be a list")
            if len(keywords) > 5:
                raise DesignSchemaError(f"Page {page.get('page')} emphasis_plan may highlight at most 5 key phrases")
            for index, keyword in enumerate(keywords, start=1):
                if not isinstance(keyword, dict):
                    raise DesignSchemaError(f"Page {page.get('page')} emphasis keyword {index} must be an object")
                cls._require_fields(keyword, PAGE_EMPHASIS_KEYWORD_FIELDS, f"Page {page.get('page')} emphasis keyword {index}")
                text = str(keyword.get("text") or "").strip()
                if len(text) > 24:
                    raise DesignSchemaError(f"Page {page.get('page')} emphasis keyword {index} must be a short phrase")
            visible_text = page.get("visible_text")
            if not isinstance(visible_text, list) or any(not isinstance(item, str) or len(item.strip()) > 120 for item in visible_text):
                raise DesignSchemaError(f"Page {page.get('page')} visible_text must be short strings")
            prompt = page.get("implement_prompt")
            if not isinstance(prompt, str) or len(prompt.strip()) < 80:
                raise DesignSchemaError(f"Page {page.get('page')} missing usable page-specific implement_prompt")
            if API_OUTPUT_PROMPT_PATTERN.search(prompt) or OUTPUT_SETTINGS_SENTENCE_PATTERN.search(prompt):
                raise DesignSchemaError(f"Page {page.get('page')} implement_prompt must not include API output settings")
            cls._validate_no_copy_request(prompt)
        return sorted_pages

    @classmethod
    def _sanitize_ppt_page_prompt(cls, prompt: str) -> str:
        cleaned = OUTPUT_SETTINGS_SENTENCE_PATTERN.sub("", prompt)
        cleaned = API_OUTPUT_PROMPT_PATTERN.sub("", cleaned)
        cleaned = re.sub(r"\s{2,}", " ", cleaned)
        cleaned = re.sub(r"\s+([,.;:])", r"\1", cleaned)
        return cleaned.strip()

    @staticmethod
    def _stringify_master_value(value: Any) -> str:
        if isinstance(value, str):
            return value.strip()
        return json.dumps(value, ensure_ascii=False, sort_keys=True)

    @classmethod
    def _ppt_master_prompt_prefix(cls, template_analysis: dict[str, Any], page: dict[str, Any]) -> str:
        master = cls._master_style_spec(template_analysis)
        binding = page.get("master_style_binding") if isinstance(page.get("master_style_binding"), dict) else {}
        parts = [
            "Create one 16:9 academic PowerPoint-style slide.",
            "All visible slide text must be Simplified Chinese only.",
            "Use the extracted template master specification below as immutable; the implement model does not receive the template image, so these text constraints are the source of truth.",
            "Use only the page-specific title and page number supplied in the page-specific section; keep them in the extracted title and page-number regions.",
        ]
        parts.extend(
            [
                f"Background/canvas: {cls._stringify_master_value(master.get('canvas'))}",
                f"Title region: {cls._stringify_master_value(binding.get('title_region') or master.get('title_region'))}",
                f"Safe margins/body area: {cls._stringify_master_value(binding.get('safe_margins') or master.get('safe_margins'))}",
                f"Header/footer/page number/logo/corner marks: {cls._stringify_master_value(binding.get('header_footer') or master.get('header_footer'))}",
                f"Divider lines: {cls._stringify_master_value(binding.get('divider_lines') or master.get('divider_lines'))}",
                f"Palette/background colors: {cls._stringify_master_value(binding.get('palette') or master.get('palette'))}",
                f"Typography/font hierarchy: {cls._stringify_master_value(binding.get('typography') or master.get('typography'))}",
                f"Module/card/border style: {cls._stringify_master_value(binding.get('module_style') or master.get('module_style'))}",
                f"Decorative/immutable elements: {cls._stringify_master_value(master.get('decorative_elements'))}; {cls._stringify_master_value(master.get('immutable_elements'))}",
                f"Forbidden deviations: {cls._stringify_master_value(master.get('forbidden_deviations'))}",
                "Do not add API output settings to the prompt text. Do not add extra page numbers, random logos, new corner marks, unrelated footer citations, gradients, editing grids, or decorative noise.",
            ]
        )
        return "\n".join(part for part in parts if part and part != "None")

    @classmethod
    def _apply_ppt_master_prompt_prefix(cls, pages: list[dict[str, Any]], template_analysis: dict[str, Any]) -> list[dict[str, Any]]:
        updated: list[dict[str, Any]] = []
        for page in pages:
            page_prompt = cls._sanitize_ppt_page_prompt(str(page.get("implement_prompt") or ""))
            prefix = cls._ppt_master_prompt_prefix(template_analysis, page)
            page_number = page.get("page")
            title = str(page.get("title") or "").strip()
            page_context = [
                f"Slide title: {title}." if title else "",
                f"Use page number {page_number} only in the extracted page-number position." if page_number else "",
                f"Selected body skeleton: {page.get('selected_template')}. Slide type: {page.get('slide_type')}.",
                f"Body layout plan: {page.get('body_layout_plan')}.",
                f"Visual element plan: {cls._stringify_master_value(page.get('visual_element_plan'))}.",
                f"Keyword emphasis plan: {cls._stringify_master_value(page.get('emphasis_plan'))}.",
                "Use visual elements only inside the body safe area. Keep them proportional to text, aligned to the template palette, and avoid inventing real brand logos when source context is missing.",
                "Highlight only the planned key phrases using bold weight or the template primary/accent red; do not over-highlight full sentences.",
                page_prompt,
            ]
            merged_prompt = f"{prefix}\n\nPage-specific body layout and content:\n" + "\n".join(item for item in page_context if item)
            updated.append({**page, "implement_prompt": merged_prompt.strip()})
        return updated

    @staticmethod
    def _design_request_summary(
        profile,
        prompt_assets: list[dict[str, str]],
        prompt: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None = None,
    ) -> dict[str, Any]:
        summary = {
            "profile": public_profile_snapshot(profile),
            "prompt_assets": prompt_assets,
            "prompt": prompt,
            "reference_images": [{"filename": image["filename"], "mime_type": image["mime_type"]} for image in images],
        }
        if timeout_seconds is not None:
            summary["timeout_seconds"] = timeout_seconds
        return summary

    @staticmethod
    def _implement_request_summary(
        profile,
        prompt: str,
        output_overrides: dict[str, Any],
        reference_count: int,
        page: int | None = None,
    ) -> dict[str, Any]:
        summary = {
            "profile": public_profile_snapshot(profile),
            "prompt": prompt,
            "output_overrides": output_overrides,
            "reference_image_count": reference_count,
        }
        if page is not None:
            summary["page"] = page
        return summary

    @staticmethod
    def _image_response_summary(image: JobImage, image_b64: str, page: int | None = None) -> dict[str, Any]:
        summary: dict[str, Any] = {
            "name": image.name,
            "url": image.url,
            "bytes": len(base64.b64decode(image_b64)),
        }
        if page is not None:
            summary["page"] = page
        return summary

    @staticmethod
    def _ppt_output_defaults(profile) -> dict[str, Any]:
        protocol = "banana2" if profile.protocol == "banna2" else profile.protocol
        defaults = {key: value for key, value in profile.output_defaults.items() if value not in (None, "")}
        if protocol == "image2":
            return {**IMAGE2_FALLBACK_OUTPUT, **defaults}
        if protocol == "banana2":
            return {
                "aspect_ratio": defaults.get("aspect_ratio", "16:9"),
                "image_size": defaults.get("image_size", "4K"),
                "thinking_level": defaults.get("thinking_level", "high"),
                "mime_type": defaults.get("mime_type", "image/png"),
            }
        return defaults

    @staticmethod
    def _ppt_output_prompt_text(output: dict[str, Any]) -> str:
        return ", ".join(f"{key}={value}" for key, value in output.items() if value not in (None, "")) or "use implement model defaults"

    @staticmethod
    def _paper_contract() -> str:
        return """Return strict JSON only. Choose diagram/workflow/comparison/mechanism OR plot/chart and include the matching spec:
{
  "figure": {
    "title": "...",
    "visual_type": "diagram|workflow|comparison|mechanism|plot|chart",
    "aspect_ratio": "inherit|16:9|4:3|1:1|3:2|2:3|9:16",
    "template_usage": "strict|balanced|loose",
    "layout_constraints": ["canvas, composition, hierarchy, spacing, and template-reference constraints"],
    "semantic_constraints": ["faithfulness rules grounded in the user section"],
    "visual_constraints": ["publication quality, palette, typography, contrast, readable layout"],
    "forbidden_errors": ["hallucination", "reversed flow", "scope violation", "text overload", "direct template copying"],
    "quality_rubric": {
      "faithfulness": "...",
      "conciseness": "...",
      "readability": "...",
      "aesthetics": "..."
    },
    "visible_text": ["short labels only"],
    "diagram_spec": {
      "modules": ["..."],
      "entities": ["..."],
      "connections": [{"source": "...", "target": "...", "meaning": "..."}],
      "flow_direction": "...",
      "grouping_hierarchy": "...",
      "arrow_routing": "...",
      "label_strategy": "..."
    },
    "plot_spec": {
      "chart_type": "...",
      "data_fields": ["..."],
      "axes": {"x": "label and unit", "y": "label and unit"},
      "units": "... or none",
      "series_or_categories": ["..."],
      "legend": "...",
      "statistical_annotations": "none or supported annotations only",
      "data_integrity_rules": "No value distortion, misleading scales, label fabrication, wrong chart type, or unsupported statistics."
    },
    "implement_prompt": "A complete image-generation prompt that explicitly covers publication quality, faithfulness, conciseness, readability, forbidden errors, and template boundary."
  },
  "quality_checklist": ["..."]
}
Omit `diagram_spec` only for plot/chart. Omit `plot_spec` only for diagram/workflow/comparison/mechanism."""

    @staticmethod
    def _template_analysis_contract() -> str:
        return """Return strict JSON only with professional PPT master analysis:
{
  "template_analysis": {
    "master_style_summary": "...",
    "master_style_spec": {
      "canvas": {"aspect_ratio": "16:9", "orientation": "landscape", "background": "..."},
      "title_region": {"position": "normalized coordinates", "alignment": "...", "hierarchy": "...", "reserved_whitespace": "..."},
      "safe_margins": {"top": "...", "right": "...", "bottom": "...", "left": "...", "body_area": "..."},
      "header_footer": {"page_number": "...", "logo": "...", "corner_marks": "...", "reserved_regions": ["..."]},
      "divider_lines": [{"position": "...", "stroke": "...", "color": "..."}],
      "palette": {"background": "#...", "primary": "#...", "secondary": "#...", "accent": "#...", "neutral": "#...", "forbidden_drift": "..."},
      "typography": {"title": "...", "subtitle": "...", "body": "...", "caption": "...", "alignment": "..."},
      "module_style": {"border": "...", "radius": "...", "fill": "...", "shadow": "...", "spacing": "..."},
      "decorative_elements": ["..."],
      "immutable_elements": ["title region", "page number/logo/corner marks", "divider lines", "background", "palette", "typography", "module/card style"],
      "page_layout_rules": "How A/B/C body skeletons may vary inside safe body area only.",
      "forbidden_deviations": ["moving master elements", "palette drift", "new logo/page number", "changing title region", "overflowing safe margins"]
    },
    "global_constraints": ["..."],
    "immutable_elements": ["..."],
    "page_layout_rules": "..."
  }
}"""

    @classmethod
    def _ppt_outline_contract(cls, page_count: int) -> str:
        return f"""Return strict JSON only. Deck outline mode: return exactly {page_count} lightweight page briefs and a shared prompt. Do not write page-level implement_prompt here.
{{
  "deck_outline": {{
    "deck_title": "Simplified Chinese deck title",
    "deck_goal": "What the deck must communicate to the audience.",
    "narrative_arc": "How page 1..{page_count} progress logically without repetition.",
    "shared_prompt": {{
      "audience": "target audience",
      "tone": "academic presentation tone",
      "global_style_constraints": "Use the extracted template master as immutable; body layouts vary only inside safe area.",
      "terminology": ["consistent key terms"],
      "visual_language": "Use proportionate icons, logo-like/product/object visuals when useful and source-grounded.",
      "emphasis_language": "Use bold or template accent red for 2-5 short phrases per page only."
    }},
    "page_briefs": [
      {{
        "page": 1,
        "title": "Simplified Chinese slide title",
        "role": "cover|problem|method|experiment|result|summary|transition|technical body",
        "main_message": "One-sentence page claim.",
        "content_points": ["3-5 concise content points grounded in material"],
        "suggested_template": "Template A|Template B|Template C-1|Template C-2|Template C-3|Template C-4|Template C-5|Template C-6|Template C-7",
        "visual_direction": "Suggested visual/icon/object/chart direction for this page.",
        "transition_from_previous": "How this page connects from previous page, or none for page 1.",
        "transition_to_next": "How this page leads to next page, or closure for last page."
      }}
    ]
  }}
}}"""

    @classmethod
    def _ppt_single_page_contract(cls, page_number: int) -> str:
        return f"""Return strict JSON only. Single-page worker mode: return exactly one page object for page {page_number}. Do not output other pages. Do not put API output parameters such as size, quality, output_format, response_format, aspect_ratio, image_size, thinking_level, or mime_type into prompt text.
{{
  "page": {{
    "page": {page_number},
    "selected_template": "Template A|Template B|Template C-1|Template C-2|Template C-3|Template C-4|Template C-5|Template C-6|Template C-7",
    "title": "Simplified Chinese slide title from the current page brief",
    "slide_type": "...",
    "body_layout_plan": "Describe only this page's variable body skeleton inside the safe body area, including text/visual balance.",
    "master_style_binding": {{
      "title_region": "same extracted title region and title hierarchy",
      "safe_margins": "same body safe area and no-overflow margins",
      "header_footer": "same page number/logo/corner marks/header/footer behavior",
      "divider_lines": "same divider/separator geometry and stroke",
      "palette": "same background/primary/accent/neutral colors",
      "typography": "same font hierarchy, weights, sizes, and alignment",
      "module_style": "same card/frame/border/radius/fill/shadow/spacing rhythm",
      "background": "same background treatment"
    }},
    "visual_element_plan": {{
      "usage_decision": "Use semantic icon/logo-like/product render/object visual elements when content benefits, or explicitly state none.",
      "elements": [
        {{
          "type": "icon|logo-like symbol|product/tool mark|object render|equipment illustration|material/sample image|scene illustration|none",
          "subject": "what the visual represents",
          "source_reference": "source URL/title from Visual Asset Search Context, or generic semantic icon when no source is available",
          "placement": "where it sits inside the safe body area",
          "style": "must match template palette, typography, card/border style, and academic restraint",
          "size_ratio": "small|medium|large with approximate body-area percentage"
        }}
      ],
      "text_visual_balance": "Explain how visual elements and text remain proportionate and readable."
    }},
    "emphasis_plan": {{
      "keywords": [{{"text": "short Chinese key phrase", "style": "bold|template-primary-red|template-accent-red", "reason": "why this phrase is emphasized"}}],
      "style_rules": "Highlight only 2-5 short key phrases per page; never mark whole sentences or drift from template palette."
    }},
    "visible_text": ["Simplified Chinese visible text only, short strings"],
    "implement_prompt": "Page-specific body instructions only. Start with: Create one 16:9 academic PowerPoint-style slide. Describe this page's variable body content, layout skeleton, visual/icon/object/product elements, diagrams/charts, keyword emphasis, and visible Chinese text. Do not repeat API output settings. Do not rely on the uploaded image or network images being available to the implement model."
  }}
}}"""

    @classmethod
    def _ppt_pages_contract(cls, page_count: int, output: dict[str, Any] | None = None) -> str:
        return f"""Return strict JSON only. Return exactly {page_count} pages. Every page must bind to the same template_analysis master_style_spec. Do not put API output parameters such as size, quality, output_format, response_format, aspect_ratio, image_size, thinking_level, or mime_type into any prompt text; those are supplied by API request fields.
{{
  "pages": [
    {{
      "page": 1,
      "selected_template": "Template A|Template B|Template C-1|Template C-2|Template C-3|Template C-4|Template C-5|Template C-6|Template C-7",
      "title": "...",
      "slide_type": "...",
      "body_layout_plan": "Describe the variable body skeleton inside the safe body area, including text/visual balance.",
      "master_style_binding": {{
        "title_region": "same extracted title region and title hierarchy",
        "safe_margins": "same body safe area and no-overflow margins",
        "header_footer": "same page number/logo/corner marks/header/footer behavior",
        "divider_lines": "same divider/separator geometry and stroke",
        "palette": "same background/primary/accent/neutral colors",
        "typography": "same font hierarchy, weights, sizes, and alignment",
        "module_style": "same card/frame/border/radius/fill/shadow/spacing rhythm",
        "background": "same background treatment"
      }},
      "visual_element_plan": {{
        "usage_decision": "Use semantic icon/logo-like/product render/object visual elements when content benefits, or explicitly state none when the page should remain text/chart only.",
        "elements": [
          {{
            "type": "icon|logo-like symbol|product/tool mark|object render|equipment illustration|material/sample image|scene illustration|none",
            "subject": "what the visual represents",
            "source_reference": "source URL/title from Visual Asset Search Context, or generic semantic icon when no source is available",
            "placement": "where it sits inside the safe body area",
            "style": "must match template palette, typography, card/border style, and academic restraint",
            "size_ratio": "small|medium|large with approximate body-area percentage"
          }}
        ],
        "text_visual_balance": "Explain how visual elements and text remain proportionate and readable."
      }},
      "emphasis_plan": {{
        "keywords": [{{"text": "short Chinese key phrase", "style": "bold|template-primary-red|template-accent-red", "reason": "why this phrase is emphasized"}}],
        "style_rules": "Highlight only 2-5 short key phrases per page; never mark whole sentences or drift from template palette."
      }},
      "visible_text": ["Simplified Chinese visible text only"],
      "implement_prompt": "Page-specific body instructions only. Start with: Create one 16:9 academic PowerPoint-style slide. Describe the variable body content, layout skeleton, visual/icon/object/product elements when useful, diagrams/charts, keyword emphasis, and visible Chinese text. Do not repeat API output settings. Do not rely on the uploaded image or network images being available to the implement model. Do not invent real brand logos when the visual asset context lacks a reliable source."
    }}
  ]
}}"""
