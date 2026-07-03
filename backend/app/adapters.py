from __future__ import annotations

import asyncio
import base64
import json
import re
from typing import Any
from urllib.parse import urlparse

import httpx

from .models import ModelProfile


class ModelAdapterError(RuntimeError):
    pass


SENSITIVE_HEADER_NAMES = {"authorization", "x-api-key", "x-goog-api-key"}
RETRY_STATUS_CODES = {429, 500, 502, 503, 504}


def _redact_header_value(name: str, value: str) -> str:
    if name.lower() not in SENSITIVE_HEADER_NAMES:
        return value
    return "<redacted>"


def redact_headers(headers: dict[str, str]) -> dict[str, str]:
    return {key: _redact_header_value(key, value) for key, value in headers.items()}


def public_profile_snapshot(profile: ModelProfile) -> dict[str, Any]:
    return {
        "id": profile.id,
        "role": profile.role,
        "name": profile.name,
        "protocol": "banana2" if profile.protocol == "banna2" else profile.protocol,
        "base_url": profile.base_url,
        "model": profile.model,
        "api_version": profile.api_version,
        "headers": {key: _redact_header_value(key, value) for key, value in profile.headers.items()},
        "timeout_seconds": profile.timeout_seconds,
        "max_retries": profile.max_retries,
        "output_defaults": profile.output_defaults,
        "has_api_key": bool(profile.api_key),
    }


def normalize_base_url(base_url: str, protocol: str) -> str:
    base = base_url.rstrip("/")
    if protocol in {"banana2", "banna2"}:
        return base
    parsed = urlparse(base)
    if re.search(r"/v\d+(?:beta)?(?:/|$)", parsed.path):
        return base
    return f"{base}/v1"


def require_api_key(profile: ModelProfile) -> str:
    if not profile.api_key:
        raise ModelAdapterError(f"{profile.name} 缺少 API key")
    return profile.api_key


def safe_error_message(error: Exception) -> str:
    message = str(error).strip()
    if not message:
        message = error.__class__.__name__
    else:
        message = f"{error.__class__.__name__}: {message}"
    message = re.sub(r"Bearer\s+[A-Za-z0-9._~+/=-]+", "Bearer <redacted>", message)
    message = re.sub(r"(?i)(api[_-]?key|x-api-key|x-goog-api-key)(['\"\s:=]+)([^'\"\s,&}]+)", r"\1\2<redacted>", message)
    return message[:800]


def data_url(mime_type: str, b64: str) -> str:
    return f"data:{mime_type};base64,{b64}"


def image_to_b64(path) -> str:
    return base64.b64encode(path.read_bytes()).decode("ascii")


def create_async_client(timeout: httpx.Timeout, proxy_url: str | None = None) -> httpx.AsyncClient:
    if proxy_url:
        return httpx.AsyncClient(timeout=timeout, proxy=proxy_url)
    return httpx.AsyncClient(timeout=timeout)


async def post_json_with_retries(
    profile: ModelProfile,
    url: str,
    payload: dict[str, Any],
    headers: dict[str, str],
    timeout_seconds: int | None = None,
    proxy_url: str | None = None,
) -> httpx.Response:
    merged_headers = {"Content-Type": "application/json", **profile.headers, **headers}
    attempts = max(1, profile.max_retries + 1)
    effective_timeout = timeout_seconds or profile.timeout_seconds
    timeout = httpx.Timeout(effective_timeout)
    last_error: httpx.TimeoutException | httpx.TransportError | None = None

    for attempt in range(attempts):
        try:
            async with create_async_client(timeout, proxy_url) as client:
                response = await client.post(url, json=payload, headers=merged_headers)
            if response.status_code not in RETRY_STATUS_CODES or attempt == attempts - 1:
                return response
        except httpx.TimeoutException as exc:
            last_error = exc
            if attempt == attempts - 1:
                break
        except httpx.TransportError as exc:
            last_error = exc
            if attempt == attempts - 1:
                break
        await asyncio.sleep(min(2**attempt, 8))

    if isinstance(last_error, httpx.TimeoutException):
        raise ModelAdapterError(f"模型请求超时：{int(effective_timeout)} 秒内未收到响应") from last_error
    if last_error is not None:
        raise ModelAdapterError(f"模型请求网络错误：{last_error.__class__.__name__}") from last_error
    raise ModelAdapterError("模型请求失败：未收到有效响应")


class DesignClient:
    async def generate(
        self,
        profile: ModelProfile,
        system_prompt: str,
        user_prompt: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None = None,
        proxy_url: str | None = None,
    ) -> str:
        protocol = "banana2" if profile.protocol == "banna2" else profile.protocol
        if protocol == "openai_chat":
            return await self._openai_chat(profile, system_prompt, user_prompt, images, timeout_seconds, proxy_url)
        if protocol == "openai_responses":
            return await self._openai_responses(profile, system_prompt, user_prompt, images, timeout_seconds, proxy_url)
        if protocol == "anthropic_messages":
            return await self._anthropic_messages(profile, system_prompt, user_prompt, images, timeout_seconds, proxy_url)
        raise ModelAdapterError(f"Unsupported design protocol: {profile.protocol}")

    async def _openai_chat(
        self,
        profile: ModelProfile,
        system_prompt: str,
        user_prompt: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None,
        proxy_url: str | None,
    ) -> str:
        url = f"{normalize_base_url(profile.base_url, profile.protocol)}/chat/completions"
        content: list[dict[str, Any]] = [{"type": "text", "text": user_prompt}]
        content.extend({"type": "image_url", "image_url": {"url": data_url(img["mime_type"], img["b64"])}} for img in images)
        payload = {
            "model": profile.model,
            "messages": [{"role": "system", "content": system_prompt}, {"role": "user", "content": content}],
            "temperature": 0.2,
        }
        data = await self._post_json(profile, url, payload, {"Authorization": f"Bearer {require_api_key(profile)}"}, timeout_seconds, proxy_url)
        return data["choices"][0]["message"]["content"]

    async def _openai_responses(
        self,
        profile: ModelProfile,
        system_prompt: str,
        user_prompt: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None,
        proxy_url: str | None,
    ) -> str:
        url = f"{normalize_base_url(profile.base_url, profile.protocol)}/responses"
        content: list[dict[str, Any]] = [{"type": "input_text", "text": user_prompt}]
        content.extend({"type": "input_image", "image_url": data_url(img["mime_type"], img["b64"])} for img in images)
        payload = {
            "model": profile.model,
            "instructions": system_prompt,
            "input": [{"role": "user", "content": content}],
            "text": {"format": {"type": "text"}},
        }
        data = await self._post_json(profile, url, payload, {"Authorization": f"Bearer {require_api_key(profile)}"}, timeout_seconds, proxy_url)
        if data.get("output_text"):
            return data["output_text"]
        for item in data.get("output", []):
            for part in item.get("content", []):
                if part.get("type") == "output_text":
                    return part.get("text", "")
        raise ModelAdapterError("OpenAI Responses result did not contain output text")

    async def _anthropic_messages(
        self,
        profile: ModelProfile,
        system_prompt: str,
        user_prompt: str,
        images: list[dict[str, str]],
        timeout_seconds: int | None,
        proxy_url: str | None,
    ) -> str:
        url = f"{normalize_base_url(profile.base_url, profile.protocol)}/messages"
        content: list[dict[str, Any]] = [{"type": "text", "text": user_prompt}]
        content.extend(
            {
                "type": "image",
                "source": {"type": "base64", "media_type": img["mime_type"], "data": img["b64"]},
            }
            for img in images
        )
        payload = {"model": profile.model, "max_tokens": 4096, "system": system_prompt, "messages": [{"role": "user", "content": content}]}
        data = await self._post_json(
            profile,
            url,
            payload,
            {"x-api-key": require_api_key(profile), "anthropic-version": profile.api_version or "2023-06-01"},
            timeout_seconds,
            proxy_url,
        )
        return "\n".join(part.get("text", "") for part in data.get("content", []) if part.get("type") == "text")

    async def _post_json(
        self,
        profile: ModelProfile,
        url: str,
        payload: dict[str, Any],
        headers: dict[str, str],
        timeout_seconds: int | None = None,
        proxy_url: str | None = None,
    ) -> dict[str, Any]:
        response = await post_json_with_retries(profile, url, payload, headers, timeout_seconds, proxy_url)
        if response.status_code >= 400:
            raise ModelAdapterError(f"Model request failed: HTTP {response.status_code} {response.text[:400]}")
        return response.json()


class ImplementClient:
    async def generate(
        self,
        profile: ModelProfile,
        prompt: str,
        reference_images: list[dict[str, str]] | None = None,
        output_overrides: dict[str, Any] | None = None,
        proxy_url: str | None = None,
    ) -> str:
        protocol = "banana2" if profile.protocol == "banna2" else profile.protocol
        if protocol == "image2":
            return await self._image2(profile, prompt, reference_images or [], output_overrides or {}, proxy_url)
        if protocol == "banana2":
            return await self._banana2(profile, prompt, reference_images or [], output_overrides or {}, proxy_url)
        raise ModelAdapterError(f"Unsupported implement protocol: {profile.protocol}")

    async def _image2(
        self,
        profile: ModelProfile,
        prompt: str,
        reference_images: list[dict[str, str]],
        output_overrides: dict[str, Any],
        proxy_url: str | None,
    ) -> str:
        base = normalize_base_url(profile.base_url, "image2")
        defaults = {key: value for key, value in {**profile.output_defaults, **output_overrides}.items() if value not in (None, "")}
        headers = {"Authorization": f"Bearer {require_api_key(profile)}"}
        image_fields = self._image2_fields(defaults)
        if reference_images:
            url = f"{base}/images/edits"
            files = [("image", (img["filename"], base64.b64decode(img["b64"]), img["mime_type"])) for img in reference_images]
            data = {"model": profile.model, "prompt": prompt, **{key: str(value) for key, value in image_fields.items()}}
            async with create_async_client(httpx.Timeout(profile.timeout_seconds), proxy_url) as client:
                response = await client.post(url, data=data, files=files, headers={**profile.headers, **headers})
        else:
            url = f"{base}/images/generations"
            payload = {"model": profile.model, "prompt": prompt, **image_fields}
            response = await post_json_with_retries(profile, url, payload, headers, proxy_url=proxy_url)
        if response.status_code >= 400:
            raise ModelAdapterError(f"Image request failed: HTTP {response.status_code} {response.text[:400]}")
        data = response.json()
        image = data.get("data", [{}])[0]
        if image.get("b64_json"):
            return image["b64_json"]
        if image.get("url"):
            async with create_async_client(httpx.Timeout(profile.timeout_seconds), proxy_url) as client:
                image_response = await client.get(image["url"])
            if image_response.status_code >= 400:
                raise ModelAdapterError(f"Image download failed: HTTP {image_response.status_code}")
            return base64.b64encode(image_response.content).decode("ascii")
        raise ModelAdapterError("Image response did not contain b64_json or url")

    @staticmethod
    def _image2_fields(defaults: dict[str, Any]) -> dict[str, Any]:
        fields: dict[str, Any] = {
            "size": defaults.get("size") or "1200x675",
            "quality": defaults.get("quality") or "auto",
            "n": int(defaults.get("n") or 1),
            "response_format": defaults.get("response_format") or "b64_json",
        }
        for key in ("background", "moderation", "output_format", "output_compression", "user"):
            if defaults.get(key) not in (None, ""):
                fields[key] = defaults[key]
        return fields

    async def _banana2(
        self,
        profile: ModelProfile,
        prompt: str,
        reference_images: list[dict[str, str]],
        output_overrides: dict[str, Any],
        proxy_url: str | None,
    ) -> str:
        defaults = {**profile.output_defaults, **output_overrides}
        version = profile.api_version or "v1beta"
        url = f"{profile.base_url.rstrip('/')}/{version}/interactions"
        input_blocks: list[dict[str, Any]] = []
        input_blocks.extend({"type": "image", "mime_type": img["mime_type"], "data": img["b64"]} for img in reference_images)
        input_blocks.append({"type": "text", "text": prompt})
        payload = {
            "model": profile.model,
            "input": input_blocks,
            "response_format": {
                "type": "image",
                "aspect_ratio": defaults.get("aspect_ratio", "16:9"),
                "image_size": defaults.get("image_size", "4K"),
            },
            "generation_config": {"thinking_level": defaults.get("thinking_level", "high")},
        }
        headers = {"x-goog-api-key": require_api_key(profile)}
        response = await post_json_with_retries(profile, url, payload, headers, proxy_url=proxy_url)
        if response.status_code >= 400:
            raise ModelAdapterError(f"Gemini image request failed: HTTP {response.status_code} {response.text[:400]}")
        data = response.json()
        image = self._extract_gemini_image(data)
        if not image:
            raise ModelAdapterError("Gemini response did not contain image data")
        return image

    @staticmethod
    def _extract_gemini_image(data: dict[str, Any]) -> str | None:
        for step in data.get("steps", []):
            if step.get("type") != "model_output":
                continue
            for block in step.get("content", []):
                if block.get("type") == "image" and block.get("data"):
                    return block["data"]
        for candidate in data.get("candidates", []):
            for part in candidate.get("content", {}).get("parts", []):
                inline_data = part.get("inlineData") or part.get("inline_data")
                if inline_data and inline_data.get("data"):
                    return inline_data["data"]
        return None


def parse_json_response(text: str) -> dict[str, Any]:
    cleaned = text.strip()
    cleaned = re.sub(r"^```(?:json)?", "", cleaned).strip()
    cleaned = re.sub(r"```$", "", cleaned).strip()
    try:
        return json.loads(cleaned)
    except json.JSONDecodeError:
        start = cleaned.find("{")
        end = cleaned.rfind("}")
        if start >= 0 and end > start:
            return json.loads(cleaned[start : end + 1])
        raise
