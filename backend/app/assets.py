from __future__ import annotations

import json
import mimetypes
import uuid
from pathlib import Path

from fastapi import HTTPException, UploadFile
from fastapi.responses import FileResponse

from .config import app_home
from .models import AssetUploadResponse


class AssetStore:
    def __init__(self, root: Path | None = None) -> None:
        self.root = root or app_home() / "assets"
        self.index_path = self.root / "index.json"

    async def save_upload(self, upload: UploadFile) -> AssetUploadResponse:
        self.root.mkdir(parents=True, exist_ok=True)
        suffix = Path(upload.filename or "upload").suffix or ".bin"
        asset_id = uuid.uuid4().hex
        filename = f"{asset_id}{suffix}"
        path = self.root / filename
        data = await upload.read()
        path.write_bytes(data)
        mime_type = upload.content_type or mimetypes.guess_type(filename)[0] or "application/octet-stream"
        index = self._load_index()
        index[asset_id] = {"filename": upload.filename or filename, "path": filename, "mime_type": mime_type}
        self.index_path.write_text(json.dumps(index, ensure_ascii=False, indent=2), encoding="utf-8")
        return AssetUploadResponse(id=asset_id, filename=upload.filename or filename, mime_type=mime_type, url=f"/api/assets/{asset_id}")

    def metadata(self, asset_id: str) -> dict[str, str]:
        index = self._load_index()
        if asset_id not in index:
            raise HTTPException(status_code=404, detail="Asset not found")
        return index[asset_id]

    def get(self, asset_id: str) -> tuple[Path, str]:
        item = self.metadata(asset_id)
        return self.root / item["path"], item.get("mime_type") or "application/octet-stream"

    def response(self, asset_id: str) -> FileResponse:
        path, mime_type = self.get(asset_id)
        if not path.exists():
            raise HTTPException(status_code=404, detail="Asset file not found")
        return FileResponse(path, media_type=mime_type)

    def _load_index(self) -> dict[str, dict[str, str]]:
        if not self.index_path.exists():
            return {}
        return json.loads(self.index_path.read_text(encoding="utf-8"))

