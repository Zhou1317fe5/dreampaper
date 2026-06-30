from __future__ import annotations

import json
import os
from pathlib import Path

from .models import AppConfig, ModelProfile, PublicAppConfig, PublicModelProfile


def app_home() -> Path:
    return Path(os.getenv("DREAMPAPER_HOME", "~/.dreampaper")).expanduser()


def default_config() -> AppConfig:
    return AppConfig(
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
                timeout_seconds=300,
                max_retries=1,
                output_defaults={
                    "size": "3840x2160",
                    "quality": "high",
                    "output_format": "png",
                    "aspect_ratio": "16:9",
                    "image_size": "4K",
                    "thinking_level": "high",
                    "mime_type": "image/png",
                },
            ),
        ]
    )


class ConfigStore:
    def __init__(self, path: Path | None = None) -> None:
        self.path = path or app_home() / "config.json"

    def load(self) -> AppConfig:
        if not self.path.exists():
            return default_config()
        data = json.loads(self.path.read_text(encoding="utf-8"))
        return AppConfig.model_validate(data)

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
        saved = incoming.model_copy(update={"model_profiles": profiles})
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
            model_profiles=[self._public_profile(profile) for profile in config.model_profiles],
        )

    def active_profile(self, role: str) -> ModelProfile:
        config = self.load()
        active_id = config.active_design_profile if role == "design" else config.active_implement_profile
        for profile in config.model_profiles:
            if profile.id == active_id and profile.role == role:
                return profile
        for profile in config.model_profiles:
            if profile.role == role:
                return profile
        raise ValueError(f"Missing active {role} model profile")

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

