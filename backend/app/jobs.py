from __future__ import annotations

import base64
import json
import mimetypes
import uuid
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from .adapters import DesignClient, ImplementClient, image_to_b64, parse_json_response, public_profile_snapshot, safe_error_message
from .assets import AssetStore
from .config import ConfigStore, app_home
from .models import JobCreateRequest, JobImage, JobRecord, PaperFigurePayload, PptSlidePayload
from .prompts import PromptStore, compose_prompt
from .templates import TemplateStore


PPT_OUTPUT_OVERRIDES = {
    "size": "3840x2160",
    "aspect_ratio": "16:9",
    "image_size": "4K",
}


FORBIDDEN_TEMPLATE_COPY_PHRASES = (
    "copy the template",
    "duplicate the template",
    "trace the template",
    "use the template as a base image",
    "edit the template image",
    "background edit",
)


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
        record = JobRecord(id=job_id, mode=request.mode, status="queued", created_at=timestamp, updated_at=timestamp)
        self.jobs[job_id] = record
        self._persist(record)
        return record

    def get(self, job_id: str) -> JobRecord:
        if job_id in self.jobs:
            return self.jobs[job_id]
        path = self.root / job_id / "job.json"
        if path.exists():
            record = JobRecord.model_validate_json(path.read_text(encoding="utf-8"))
            self.jobs[job_id] = record
            return record
        raise KeyError(job_id)

    async def run(self, job_id: str, request: JobCreateRequest) -> None:
        self._update(job_id, status="running", message="running")
        try:
            if request.mode == "paper_figure":
                await self._run_paper(job_id, PaperFigurePayload.model_validate(request.payload))
            else:
                await self._run_ppt(job_id, PptSlidePayload.model_validate(request.payload))
        except Exception as exc:
            record = self.get(job_id)
            artifacts = dict(record.internal_artifacts)
            artifacts["error"] = {"message": safe_error_message(exc), "failed_at": now_iso()}
            self._update(job_id, status="failed", message=safe_error_message(exc), internal_artifacts=artifacts)

    async def _run_paper(self, job_id: str, payload: PaperFigurePayload) -> None:
        if not payload.template_ids:
            raise ValueError("Paper figure requires at least one template")
        design_profile = self.config.active_profile("design")
        implement_profile = self.config.active_profile("implement")
        selected = [self.templates.get(template_id) for template_id in payload.template_ids[:3]]
        selected_template_metadata = [self._template_metadata(item) for item in selected]
        template_images = [self._template_image_payload(item) for item in selected]
        template_summary = json.dumps(selected_template_metadata, ensure_ascii=False, indent=2)
        assets = [
            self.prompts.load("global/system.md"),
            self.prompts.load("global/figure_style.md"),
            self.prompts.load("modes/paper_figure/design.md"),
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
        user_prompt, prompt_assets = compose_prompt(
            assets[:3],
            {
                "Selected Template Metadata": template_summary,
                "User Input": json.dumps(user_context, ensure_ascii=False, indent=2),
                "Output Contract": self._paper_contract(),
            },
        )
        prompt_assets.append({key: assets[3][key] for key in ("key", "version", "hash")})
        design_request = self._design_request_summary(design_profile, prompt_assets, user_prompt, template_images)
        self._merge_artifacts(
            job_id,
            {
                "normalized_input": user_context,
                "selected_templates": selected_template_metadata,
                "prompt_assets": prompt_assets,
                "design_model_request": design_request,
            },
        )
        design_text = await self.design.generate(design_profile, assets[0]["content"], user_prompt, template_images)
        design_json = await self._parse_or_repair(design_profile, assets[0]["content"], user_prompt, design_text, template_images)
        self._validate_paper_design(design_json)
        implement_prompt = self._paper_implement_prompt(design_json)
        image_b64 = await self.implement.generate(implement_profile, implement_prompt)
        image = self._save_image(job_id, "paper_figure.png", image_b64)
        self._update(
            job_id,
            status="succeeded",
            message="completed",
            images=[image],
            internal_artifacts={
                **self.get(job_id).internal_artifacts,
                "design_model_response": {"raw_text": design_text, "parsed_json": design_json},
                "design_response": design_json,
                "implement_model_request": self._implement_request_summary(implement_profile, implement_prompt, {}, 0),
                "implement_model_response": self._image_response_summary(image, image_b64),
                "implement_prompts": [implement_prompt],
                "retries": {"design_json_repair": "attempted only on parse failure"},
            },
        )

    async def _run_ppt(self, job_id: str, payload: PptSlidePayload) -> None:
        design_profile = self.config.active_profile("design")
        implement_profile = self.config.active_profile("implement")
        template_path, template_mime = self.assets.get(payload.template_asset_id)
        template_image = {"filename": template_path.name, "mime_type": template_mime, "b64": image_to_b64(template_path)}
        normalized_input = {
            "template_asset_id": payload.template_asset_id,
            "material_text": payload.material_text.strip(),
            "page_count": payload.page_count,
            "custom_prompt": payload.custom_prompt or "None",
            "output": PPT_OUTPUT_OVERRIDES,
        }
        analyzer_assets = [self.prompts.load("global/system.md"), self.prompts.load("modes/ppt_slide/analyzer.md")]
        analyzer_prompt, analyzer_prompt_assets = compose_prompt(
            analyzer_assets,
            {"Output Contract": self._template_analysis_contract()},
        )
        self._merge_artifacts(
            job_id,
            {
                "normalized_input": normalized_input,
                "prompt_assets": analyzer_prompt_assets,
                "template_asset_id": payload.template_asset_id,
                "design_model_request": self._design_request_summary(design_profile, analyzer_prompt_assets, analyzer_prompt, [template_image]),
            },
        )
        analysis_text = await self.design.generate(design_profile, analyzer_assets[0]["content"], analyzer_prompt, [template_image])
        template_analysis = await self._parse_or_repair(design_profile, analyzer_assets[0]["content"], analyzer_prompt, analysis_text, [template_image])
        page_assets = [
            self.prompts.load("global/system.md"),
            self.prompts.load("modes/ppt_slide/design.md"),
            self.prompts.load("styles/academic_ppt.md"),
        ]
        page_prompt, page_prompt_assets = compose_prompt(
            page_assets,
            {
                "Template Analysis": json.dumps(template_analysis, ensure_ascii=False, indent=2),
                "Material": payload.material_text,
                "Page Count": payload.page_count,
                "Custom Prompt": payload.custom_prompt or "None",
                "Output Contract": self._ppt_pages_contract(payload.page_count),
            },
        )
        all_prompt_assets = analyzer_prompt_assets + page_prompt_assets
        self._merge_artifacts(
            job_id,
            {
                "template_analysis": template_analysis,
                "prompt_assets": all_prompt_assets,
                "design_model_response": {"template_analysis_raw_text": analysis_text, "template_analysis_json": template_analysis},
                "page_design_model_request": self._design_request_summary(design_profile, page_prompt_assets, page_prompt, [template_image]),
            },
        )
        pages_text = await self.design.generate(design_profile, page_assets[0]["content"], page_prompt, [template_image])
        pages_json = await self._parse_or_repair(design_profile, page_assets[0]["content"], page_prompt, pages_text, [template_image])
        pages = self._validate_ppt_pages(pages_json, payload.page_count)
        images: list[JobImage] = []
        implement_prompts: list[str] = []
        implement_requests: list[dict[str, Any]] = []
        implement_responses: list[dict[str, Any]] = []
        for page in pages:
            prompt = page["implement_prompt"]
            image_b64 = await self.implement.generate(implement_profile, prompt, output_overrides=PPT_OUTPUT_OVERRIDES)
            image = self._save_image(job_id, f"slide_{page['page']}.png", image_b64)
            images.append(image)
            implement_prompts.append(prompt)
            implement_requests.append(self._implement_request_summary(implement_profile, prompt, PPT_OUTPUT_OVERRIDES, 0, page=page["page"]))
            implement_responses.append(self._image_response_summary(image, image_b64, page=page["page"]))
        self._update(
            job_id,
            status="succeeded",
            message="completed",
            images=images,
            internal_artifacts={
                **self.get(job_id).internal_artifacts,
                "page_design_model_response": {"raw_text": pages_text, "parsed_json": pages_json},
                "page_plan": pages,
                "design_response": {"template_analysis": template_analysis, "pages": pages},
                "implement_model_request": implement_requests,
                "implement_model_response": implement_responses,
                "implement_prompts": implement_prompts,
                "retries": {"design_json_repair": "attempted only on parse failure"},
            },
        )

    async def _parse_or_repair(self, profile, system_prompt: str, original_prompt: str, text: str, images: list[dict[str, str]]) -> dict[str, Any]:
        try:
            return parse_json_response(text)
        except Exception:
            repair_prompt = (
                "The previous model output was not valid JSON. Convert it into strict JSON only, preserving all useful content.\n\n"
                f"Original task:\n{original_prompt}\n\nInvalid output:\n{text}"
            )
            repaired = await self.design.generate(profile, system_prompt, repair_prompt, images)
            return parse_json_response(repaired)

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
            raise ValueError("Design model response missing implement_prompt")
        return prompt

    @classmethod
    def _validate_paper_design(cls, design_json: dict[str, Any]) -> None:
        figure = design_json.get("figure") if isinstance(design_json.get("figure"), dict) else design_json
        if not isinstance(figure, dict):
            raise ValueError("Design model response must be a JSON object")
        implement_prompt = cls._paper_implement_prompt(design_json)
        if not isinstance(implement_prompt, str) or len(implement_prompt.strip()) < 80:
            raise ValueError("Design model response missing a usable implement_prompt")
        aspect_ratio = figure.get("aspect_ratio")
        if aspect_ratio and aspect_ratio not in {"inherit", "16:9", "4:3", "1:1", "3:2", "2:3", "9:16"}:
            raise ValueError(f"Invalid paper figure aspect_ratio: {aspect_ratio}")
        visible_text = figure.get("visible_text", [])
        if not isinstance(visible_text, list) or any(not isinstance(item, str) or len(item) > 80 for item in visible_text):
            raise ValueError("Paper figure visible_text must be short label strings")
        lowered_prompt = implement_prompt.lower()
        if any(phrase in lowered_prompt for phrase in FORBIDDEN_TEMPLATE_COPY_PHRASES):
            raise ValueError("Implement prompt requests direct template copying or editing")

    @staticmethod
    def _validate_ppt_pages(pages_json: dict[str, Any], page_count: int) -> list[dict[str, Any]]:
        pages = pages_json.get("pages") or []
        if not isinstance(pages, list):
            raise ValueError("PPT design response pages must be a list")
        if len(pages) != page_count:
            raise ValueError(f"Design model returned {len(pages)} pages, expected {page_count}")
        sorted_pages = sorted(pages, key=lambda item: item.get("page", 0))
        expected_pages = list(range(1, page_count + 1))
        actual_pages = [item.get("page") for item in sorted_pages]
        if actual_pages != expected_pages:
            raise ValueError(f"PPT pages must be ordered 1..{page_count}; got {actual_pages}")
        for page in sorted_pages:
            prompt = page.get("implement_prompt")
            if not isinstance(prompt, str) or len(prompt.strip()) < 120:
                raise ValueError(f"Page {page.get('page')} missing a usable implement_prompt")
        return sorted_pages

    @staticmethod
    def _design_request_summary(profile, prompt_assets: list[dict[str, str]], prompt: str, images: list[dict[str, str]]) -> dict[str, Any]:
        return {
            "profile": public_profile_snapshot(profile),
            "prompt_assets": prompt_assets,
            "prompt": prompt,
            "reference_images": [{"filename": image["filename"], "mime_type": image["mime_type"]} for image in images],
        }

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
    def _paper_contract() -> str:
        return """Return strict JSON only:
{
  "figure": {
    "title": "...",
    "visual_type": "diagram|plot|workflow|comparison|mechanism",
    "aspect_ratio": "16:9",
    "template_usage": "strict|balanced|loose",
    "layout_plan": "...",
    "visible_text": ["short labels only"],
    "implement_prompt": "A complete image-generation prompt..."
  },
  "quality_checklist": ["..."]
}"""

    @staticmethod
    def _template_analysis_contract() -> str:
        return """Return strict JSON only with:
{
  "template_analysis": {
    "master_style_summary": "...",
    "title_region": "...",
    "divider_lines": ["..."],
    "page_number": "...",
    "reserved_regions": ["..."],
    "palette": ["#..."],
    "typography": "...",
    "layout_rules": "..."
  }
}"""

    @staticmethod
    def _ppt_pages_contract(page_count: int) -> str:
        return f"""Return strict JSON only. Return exactly {page_count} pages:
{{
  "pages": [
    {{
      "page": 1,
      "selected_template": "Template A|Template B|Template C-1|Template C-2|Template C-3|Template C-4|Template C-5|Template C-6|Template C-7",
      "title": "...",
      "slide_type": "...",
      "master_style_application": "...",
      "visible_text": ["..."],
      "implement_prompt": "A complete 4K 3840x2160 16:9 image-generation prompt that preserves the uploaded master template elements..."
    }}
  ]
}}"""
