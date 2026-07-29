from __future__ import annotations

import json
import os
from pathlib import Path

from .models import AppConfig, ModelProfile, PublicAppConfig, PublicModelProfile


def app_home() -> Path:
    return Path(os.getenv("DREAMPAPER_HOME", "~/.dreampaper")).expanduser()


DEFAULT_PROXY_URL = "http://127.0.0.1:7890"


def normalize_proxy_url(proxy_url: str | None) -> str | None:
    value = (proxy_url or "").strip()
    if not value:
        return None
    if "://" not in value:
        if value.isdigit():
            return f"http://127.0.0.1:{value}"
        return f"http://{value}"
    return value


def default_search_profile() -> ModelProfile:
    return ModelProfile(
        id="search-default",
        role="search",
        name="Search model",
        protocol="duckduckgo_html",
        base_url="https://duckduckgo.com",
        model="duckduckgo-html",
        timeout_seconds=15,
        max_retries=1,
        output_defaults={"max_results": "3"},
    )


def default_config() -> AppConfig:
    return AppConfig(
        proxy_url=DEFAULT_PROXY_URL,
        active_search_profile="search-default",
        model_profiles=[
            ModelProfile(
                id="design-default",
                role="design",
                name="Design model",
                protocol="openai_responses",
                base_url="https://api.openai.com",
                model="gpt-5.4",
                timeout_seconds=120,
                max_retries=2,
            ),
            ModelProfile(
                id="implement-default",
                role="implement",
                name="Implement model",
                protocol="image2",
                base_url="https://api.openai.com",
                model="gpt-image-2",
                timeout_seconds=600,
                max_retries=3,
                output_defaults={
                    "size": "1200x675",
                    "quality": "auto",
                    "output_format": "png",
                    "response_format": "url",
                    "aspect_ratio": "16:9",
                    "image_size": "4K",
                    "thinking_level": "high",
                    "mime_type": "image/png",
                },
            ),
            default_search_profile(),
        ]
    )


class ConfigStore:
    def __init__(self, path: Path | None = None) -> None:
        self.path = path or app_home() / "config.json"

    def load(self) -> AppConfig:
        if not self.path.exists():
            return default_config()
        data = json.loads(self.path.read_text(encoding="utf-8"))
        config = AppConfig.model_validate(data)
        return self._ensure_search_profile(config)

    def save(self, incoming: AppConfig) -> AppConfig:
        existing = self.load()
        keys_by_id = {profile.id: profile.api_key for profile in existing.model_profiles}
        profiles: list[ModelProfile] = []
        for profile in incoming.model_profiles:
            normalized_protocol = "banana2" if profile.protocol == "banna2" else profile.protocol
            next_profile = profile.model_copy(update={"protocol": normalized_protocol})
            if not next_profile.api_key:
                next_profile = next_profile.model_copy(update={"api_key": keys_by_id.get(profile.id)})
            profiles.append(next_profile)
        saved = incoming.model_copy(
            update={
                "model_profiles": profiles,
                "proxy_url": normalize_proxy_url(incoming.proxy_url),
                "active_search_profile": incoming.active_search_profile or "search-default",
            }
        )
        saved = self._ensure_search_profile(saved)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(saved.model_dump_json(indent=2), encoding="utf-8")
        try:
            self.path.chmod(0o600)
        except OSError:
            pass
        return saved

    def public(self) -> PublicAppConfig:
        config = self.load()
        return PublicAppConfig(
            version=config.version,
            active_design_profile=config.active_design_profile,
            active_implement_profile=config.active_implement_profile,
            active_search_profile=config.active_search_profile,
            proxy_url=normalize_proxy_url(config.proxy_url),
            ppt_page_plan_concurrency=config.ppt_page_plan_concurrency,
            ppt_image_concurrency=config.ppt_image_concurrency,
            model_profiles=[self._public_profile(profile) for profile in config.model_profiles],
        )

    def proxy_url(self) -> str | None:
        return normalize_proxy_url(self.load().proxy_url)

    def active_profile(self, role: str) -> ModelProfile:
        config = self.load()
        if role == "design":
            active_id = config.active_design_profile
        elif role == "implement":
            active_id = config.active_implement_profile
        elif role == "search":
            active_id = config.active_search_profile
        else:
            raise ValueError(f"Unknown model role: {role}")
        for profile in config.model_profiles:
            if profile.id == active_id and profile.role == role:
                return profile
        for profile in config.model_profiles:
            if profile.role == role:
                return profile
        if role == "search":
            return default_search_profile()
        raise ValueError(f"Missing active {role} model profile")

    @staticmethod
    def _ensure_search_profile(config: AppConfig) -> AppConfig:
        profiles = list(config.model_profiles)
        if not any(profile.role == "search" for profile in profiles):
            profiles.append(default_search_profile())
            return config.model_copy(
                update={
                    "model_profiles": profiles,
                    "active_search_profile": config.active_search_profile or "search-default",
                }
            )
        if not config.active_search_profile:
            search_id = next(profile.id for profile in profiles if profile.role == "search")
            return config.model_copy(update={"active_search_profile": search_id})
        return config

    @staticmethod
    def _public_profile(profile: ModelProfile) -> PublicModelProfile:
        hint = None
        if profile.api_key:
            hint = f"••••{profile.api_key[-4:]}" if len(profile.api_key) >= 4 else "••••"
        return PublicModelProfile(
            id=profile.id,
            role=profile.role,
            name=profile.name,
            protocol=profile.protocol,
            base_url=profile.base_url,
            model=profile.model,
            api_version=profile.api_version,
            headers=profile.headers,
            timeout_seconds=profile.timeout_seconds,
            max_retries=profile.max_retries,
            output_defaults=profile.output_defaults,
            has_api_key=bool(profile.api_key),
            api_key_hint=hint,
        )

