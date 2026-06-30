from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from fastapi import HTTPException
from fastapi.responses import FileResponse

from .models import TemplateSummary


class TemplateStore:
    def __init__(self, root: Path) -> None:
        self.root = root
        self._items: dict[str, dict[str, Any]] | None = None

    def list(self, kind: str | None = None, q: str | None = None, limit: int = 80) -> list[TemplateSummary]:
        items = self._load()
        result: list[TemplateSummary] = []
        query = (q or "").strip().lower()
        for template_id, item in items.items():
            if kind and item["kind"] != kind:
                continue
            haystack = " ".join(
                [
                    item.get("category") or "",
                    item.get("visual_intent") or "",
                    self._content_text(item.get("content"))[:500],
                ]
            ).lower()
            if query and query not in haystack:
                continue
            result.append(self._summary(template_id, item))
            if len(result) >= limit:
                break
        return result

    def image_response(self, template_id: str) -> FileResponse:
        item = self.get(template_id)
        path = self._image_path(item)
        if not path.exists():
            raise HTTPException(status_code=404, detail="Template image not found")
        return FileResponse(path)

    def get(self, template_id: str) -> dict[str, Any]:
        items = self._load()
        if template_id not in items:
            raise HTTPException(status_code=404, detail="Template not found")
        return items[template_id]

    def _load(self) -> dict[str, dict[str, Any]]:
        if self._items is not None:
            return self._items
        items: dict[str, dict[str, Any]] = {}
        for kind in ("diagram", "plot"):
            ref_path = self.root / kind / "ref.json"
            if not ref_path.exists():
                continue
            data = json.loads(ref_path.read_text(encoding="utf-8"))
            for raw in data:
                template_id = f"{kind}:{raw['id']}"
                item = dict(raw)
                item["kind"] = kind
                item["template_id"] = template_id
                items[template_id] = item
        self._items = items
        return items

    def _summary(self, template_id: str, item: dict[str, Any]) -> TemplateSummary:
        additional = item.get("additional_info") or {}
        category = item.get("category") or item.get("original_category")
        return TemplateSummary(
            id=template_id,
            source_id=str(item.get("id")),
            kind=item["kind"],
            category=category,
            rounded_ratio=additional.get("rounded_ratio"),
            visual_intent=(item.get("visual_intent") or "")[:900],
            content_summary=self._content_text(item.get("content"))[:900],
            image_url=f"/api/templates/{template_id}/image",
        )

    def _image_path(self, item: dict[str, Any]) -> Path:
        return self.root / item["kind"] / item["path_to_gt_image"]

    @staticmethod
    def _content_text(value: Any) -> str:
        if isinstance(value, str):
            return value
        return json.dumps(value, ensure_ascii=False)

