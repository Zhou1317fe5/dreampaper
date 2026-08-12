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
# 制图网关（如 WisArt）同步出图时 nginx 常在 60–300s 返回 502，需更长读超时与退避重试
DEFAULT_CONNECT_TIMEOUT_SECONDS = 30.0
MIN_IMAGE_TIMEOUT_SECONDS = 120
MAX_BACKOFF_SECONDS = 30


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


def build_timeout(timeout_seconds: int | float | None, *, minimum: int | None = None) -> httpx.Timeout:
    """分离 connect / read 超时：连接宜短，读超时覆盖同步出图等待。"""
    read = float(timeout_seconds or 120)
    if minimum is not None:
        read = max(read, float(minimum))
    return httpx.Timeout(
        connect=DEFAULT_CONNECT_TIMEOUT_SECONDS,
        read=read,
        write=min(60.0, read),
        pool=DEFAULT_CONNECT_TIMEOUT_SECONDS,
    )


def create_async_client(timeout: httpx.Timeout, proxy_url: str | None = None) -> httpx.AsyncClient:
    if proxy_url:
        return httpx.AsyncClient(timeout=timeout, proxy=proxy_url)
    return httpx.AsyncClient(timeout=timeout)


def format_transport_error(error: httpx.TransportError, proxy_url: str | None = None) -> str:
    error_type = error.__class__.__name__
    if proxy_url:
        return (
            "模型请求网络错误：无法通过已配置代理建立连接。"
            "请检查设置中的代理地址和代理服务，或清空代理后直连。"
            f"（{error_type}）"
        )
    return (
        "模型请求网络错误：无法连接模型服务。"
        "请检查模型 Base URL、DNS 和网络连接。"
        f"（{error_type}）"
    )


def retry_backoff_seconds(attempt: int, status_code: int | None = None) -> float:
    base = min(2**attempt, MAX_BACKOFF_SECONDS)
    # 网关 502/503 往往表示上游仍在出图或短暂过载，多等一会再重试
    if status_code in {502, 503, 504}:
        return min(10 * (attempt + 1), MAX_BACKOFF_SECONDS)
    if status_code == 429:
        return min(15 * (attempt + 1), MAX_BACKOFF_SECONDS)
    return float(base)


def format_http_error(kind: str, status_code: int, body: str) -> str:
    snippet = (body or "").strip().replace("\n", " ")[:280]
    if status_code == 502:
        return (
            f"{kind} failed: HTTP 502 Bad Gateway. "
            "上游网关在同步等待出图时断开（图片可能已在服务端生成）。"
            "请提高 implement 超时（建议 ≥600s）、增加重试，并优先使用 response_format=url。"
            f" body={snippet}"
        )
    if status_code in {503, 504}:
        return f"{kind} failed: HTTP {status_code}. 上游维护或超时，请稍后重试。 body={snippet}"
    return f"{kind} failed: HTTP {status_code} {snippet}"


async def post_json_with_retries(
    profile: ModelProfile,
    url: str,
    payload: dict[str, Any],
    headers: dict[str, str],
    timeout_seconds: int | None = None,
    proxy_url: str | None = None,
    *,
    minimum_timeout: int | None = None,
) -> httpx.Response:
    merged_headers = {"Content-Type": "application/json", **profile.headers, **headers}
    attempts = max(1, profile.max_retries + 1)
    effective_timeout = timeout_seconds or profile.timeout_seconds
    timeout = build_timeout(effective_timeout, minimum=minimum_timeout)
    last_error: httpx.TimeoutException | httpx.TransportError | None = None
    last_response: httpx.Response | None = None

    for attempt in range(attempts):
        try:
            async with create_async_client(timeout, proxy_url) as client:
                response = await client.post(url, json=payload, headers=merged_headers)
            last_response = response
            if response.status_code not in RETRY_STATUS_CODES or attempt == attempts - 1:
                return response
            await asyncio.sleep(retry_backoff_seconds(attempt, response.status_code))
            continue
        except httpx.TimeoutException as exc:
            last_error = exc
            if attempt == attempts - 1:
                break
        except httpx.TransportError as exc:
            last_error = exc
            if attempt == attempts - 1:
                break
        await asyncio.sleep(retry_backoff_seconds(attempt))

    if isinstance(last_error, httpx.TimeoutException):
        raise ModelAdapterError(
            f"模型请求超时：读超时 {int(timeout.read)} 秒内未收到完整响应。"
            "同步制图接口可能需要更长时间，请在 Model 配置中提高 implement 超时。"
        ) from last_error
    if last_error is not None:
        raise ModelAdapterError(format_transport_error(last_error, proxy_url)) from last_error
    if last_response is not None:
        return last_response
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
        # 同步出图可能远超 design 超时；image2 强制至少 2 分钟读超时
        timeout = build_timeout(profile.timeout_seconds, minimum=MIN_IMAGE_TIMEOUT_SECONDS)
        if reference_images:
            url = f"{base}/images/edits"
            files = [("image", (img["filename"], base64.b64decode(img["b64"]), img["mime_type"])) for img in reference_images]
            data = {"model": profile.model, "prompt": prompt, **{key: str(value) for key, value in image_fields.items()}}
            response = await self._post_multipart_with_retries(profile, url, data, files, headers, timeout, proxy_url)
        else:
            url = f"{base}/images/generations"
            payload = {"model": profile.model, "prompt": prompt, **image_fields}
            response = await post_json_with_retries(
                profile,
                url,
                payload,
                headers,
                proxy_url=proxy_url,
                minimum_timeout=MIN_IMAGE_TIMEOUT_SECONDS,
            )
        if response.status_code >= 400:
            raise ModelAdapterError(format_http_error("Image request", response.status_code, response.text))
        try:
            data = response.json()
        except Exception as exc:
            raise ModelAdapterError(f"Image response is not JSON: {response.text[:200]}") from exc
        image = data.get("data", [{}])[0] if isinstance(data.get("data"), list) and data.get("data") else {}
        if not isinstance(image, dict):
            image = {}
        if image.get("b64_json"):
            return image["b64_json"]
        if image.get("url"):
            async with create_async_client(timeout, proxy_url) as client:
                image_response = await client.get(image["url"])
            if image_response.status_code >= 400:
                raise ModelAdapterError(format_http_error("Image download", image_response.status_code, image_response.text))
            return base64.b64encode(image_response.content).decode("ascii")
        raise ModelAdapterError("Image response did not contain b64_json or url")

    async def _post_multipart_with_retries(
        self,
        profile: ModelProfile,
        url: str,
        data: dict[str, str],
        files: list[Any],
        headers: dict[str, str],
        timeout: httpx.Timeout,
        proxy_url: str | None,
    ) -> httpx.Response:
        attempts = max(1, profile.max_retries + 1)
        last_error: httpx.TimeoutException | httpx.TransportError | None = None
        last_response: httpx.Response | None = None
        for attempt in range(attempts):
            try:
                async with create_async_client(timeout, proxy_url) as client:
                    response = await client.post(url, data=data, files=files, headers={**profile.headers, **headers})
                last_response = response
                if response.status_code not in RETRY_STATUS_CODES or attempt == attempts - 1:
                    return response
                await asyncio.sleep(retry_backoff_seconds(attempt, response.status_code))
                continue
            except httpx.TimeoutException as exc:
                last_error = exc
                if attempt == attempts - 1:
                    break
            except httpx.TransportError as exc:
                last_error = exc
                if attempt == attempts - 1:
                    break
            await asyncio.sleep(retry_backoff_seconds(attempt))
        if isinstance(last_error, httpx.TimeoutException):
            raise ModelAdapterError(f"模型请求超时：读超时 {int(timeout.read)} 秒内未收到完整响应") from last_error
        if last_error is not None:
            raise ModelAdapterError(format_transport_error(last_error, proxy_url)) from last_error
        if last_response is not None:
            return last_response
        raise ModelAdapterError("模型请求失败：未收到有效响应")

    @staticmethod
    def _image2_fields(defaults: dict[str, Any]) -> dict[str, Any]:
        # 默认 url：避免同步接口回传大体积 b64 时被 nginx 502 截断（WisArt 文档支持 url / b64_json）
        fields: dict[str, Any] = {
            "size": defaults.get("size") or "1200x675",
            "quality": defaults.get("quality") or "auto",
            "n": int(defaults.get("n") or 1),
            "response_format": defaults.get("response_format") or "url",
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
        response = await post_json_with_retries(
            profile,
            url,
            payload,
            headers,
            proxy_url=proxy_url,
            minimum_timeout=MIN_IMAGE_TIMEOUT_SECONDS,
        )
        if response.status_code >= 400:
            raise ModelAdapterError(format_http_error("Gemini image request", response.status_code, response.text))
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
