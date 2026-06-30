from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any


class PromptStore:
    def __init__(self, root: Path) -> None:
        self.root = root

    def load(self, key: str) -> dict[str, str]:
        path = self.root / key
        content = path.read_text(encoding="utf-8")
        digest = hashlib.sha256(content.encode("utf-8")).hexdigest()[:16]
        return {"key": key, "version": digest, "hash": digest, "content": content}


def compose_prompt(assets: list[dict[str, str]], sections: dict[str, Any]) -> tuple[str, list[dict[str, str]]]:
    parts: list[str] = []
    for asset in assets:
        parts.append(f"## {asset['key']}\n{asset['content'].strip()}")
    for name, value in sections.items():
        parts.append(f"## {name}\n{value}")
    return "\n\n".join(parts), [{k: asset[k] for k in ("key", "version", "hash")} for asset in assets]

