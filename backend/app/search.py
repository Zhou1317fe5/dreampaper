from __future__ import annotations

import html
import json
import re
from typing import Any
from urllib.parse import parse_qs, unquote, urlparse

import httpx

from .adapters import create_async_client, post_json_with_retries, require_api_key, safe_error_message
from .models import ModelProfile

SEARCH_RESULT_LINK_PATTERN = re.compile(
    r'<a[^>]+class="[^"]*result__a[^"]*"[^>]+href="([^"]+)"[^>]*>(.*?)</a>',
    re.I | re.S,
)
SEARCH_RESULT_SNIPPET_PATTERN = re.compile(
    r'<a[^>]+class="[^"]*result__snippet[^"]*"[^>]*>(.*?)</a>|<td[^>]+class="[^"]*result-snippet[^"]*"[^>]*>(.*?)</td>',
    re.I | re.S,
)


class SearchClientError(RuntimeError):
    pass


class SearchClient:
    """可配置的检索客户端：duckduckgo_html / tavily / openai_chat（search model）。"""

    async def search(
        self,
        profile: ModelProfile,
        query: str,
        *,
        max_results: int = 3,
        proxy_url: str | None = None,
    ) -> list[dict[str, str]]:
        protocol = (profile.protocol or "duckduckgo_html").strip().lower()
        if protocol == "duckduckgo_html":
            return await self._duckduckgo_html(profile, query, max_results=max_results, proxy_url=proxy_url)
        if protocol == "tavily":
            return await self._tavily(profile, query, max_results=max_results, proxy_url=proxy_url)
        if protocol in {"openai_chat", "openai_responses"}:
            # openai_responses 走 chat 兼容路径即可
            return await self._openai_chat_search(profile, query, max_results=max_results, proxy_url=proxy_url)
        raise SearchClientError(f"Unsupported search protocol: {profile.protocol}")

    async def _duckduckgo_html(
        self,
        profile: ModelProfile,
        query: str,
        *,
        max_results: int,
        proxy_url: str | None,
    ) -> list[dict[str, str]]:
        from urllib.parse import quote_plus

        base = (profile.base_url or "https://duckduckgo.com").rstrip("/")
        if base.endswith("/html"):
            url = f"{base}/?q={quote_plus(query)}"
        else:
            url = f"{base}/html/?q={quote_plus(query)}"
        timeout = httpx.Timeout(float(profile.timeout_seconds or 15))
        headers = {"User-Agent": "Mozilla/5.0 dreampaper visual asset search", **(profile.headers or {})}
        async with create_async_client(timeout, proxy_url) as client:
            response = await client.get(url, headers=headers)
        if response.status_code >= 400:
            raise SearchClientError(f"DuckDuckGo search failed: HTTP {response.status_code}")
        return self._parse_duckduckgo_html(response.text, max_results=max_results)

    async def _tavily(
        self,
        profile: ModelProfile,
        query: str,
        *,
        max_results: int,
        proxy_url: str | None,
    ) -> list[dict[str, str]]:
        api_key = require_api_key(profile)
        base = (profile.base_url or "https://api.tavily.com").rstrip("/")
        url = f"{base}/search"
        payload = {
            "api_key": api_key,
            "query": query,
            "max_results": max_results,
            "include_answer": False,
            "search_depth": (profile.output_defaults or {}).get("search_depth") or "basic",
        }
        headers = {"Content-Type": "application/json", **(profile.headers or {})}
        timeout = httpx.Timeout(float(profile.timeout_seconds or 30))
        async with create_async_client(timeout, proxy_url) as client:
            response = await client.post(url, json=payload, headers=headers)
        if response.status_code >= 400:
            raise SearchClientError(f"Tavily search failed: HTTP {response.status_code} {response.text[:200]}")
        data = response.json()
        results: list[dict[str, str]] = []
        for item in data.get("results") or []:
            results.append(
                {
                    "title": str(item.get("title") or "").strip(),
                    "url": str(item.get("url") or "").strip(),
                    "snippet": str(item.get("content") or item.get("snippet") or "").strip()[:500],
                }
            )
            if len(results) >= max_results:
                break
        return results

    async def _openai_chat_search(
        self,
        profile: ModelProfile,
        query: str,
        *,
        max_results: int,
        proxy_url: str | None,
    ) -> list[dict[str, str]]:
        """
        Search model：OpenAI 兼容 chat 接口。
        适用于带联网能力的中转/Grok 等；模型需返回严格 JSON 结果列表。
        注意：这不是 MCP 进程内调用，而是 HTTP API（可填 xAI / 任意 OpenAI 兼容 base_url）。
        """
        from .adapters import normalize_base_url

        base = normalize_base_url(profile.base_url or "https://api.openai.com", "openai_chat")
        url = f"{base}/chat/completions"
        system = (
            "You are a web search assistant for academic slide visual grounding. "
            "Return strict JSON only: {\"results\":[{\"title\":\"\",\"url\":\"\",\"snippet\":\"\"}]}. "
            f"Return at most {max_results} results about official product/tool appearance or logo description. "
            "Prefer official docs and product pages. No markdown fences."
        )
        payload = {
            "model": profile.model or "gpt-4o-mini",
            "temperature": 0.1,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": f"Search query: {query}"},
            ],
        }
        headers = {"Authorization": f"Bearer {require_api_key(profile)}"}
        response = await post_json_with_retries(
            profile,
            url,
            payload,
            headers,
            timeout_seconds=profile.timeout_seconds,
            proxy_url=proxy_url,
        )
        if response.status_code >= 400:
            raise SearchClientError(f"Search model failed: HTTP {response.status_code} {response.text[:200]}")
        data = response.json()
        content = data.get("choices", [{}])[0].get("message", {}).get("content") or ""
        parsed = self._parse_json_results(content)
        return parsed[:max_results]

    @classmethod
    def _parse_json_results(cls, content: str) -> list[dict[str, str]]:
        text = content.strip()
        text = re.sub(r"^```(?:json)?", "", text).strip()
        text = re.sub(r"```$", "", text).strip()
        try:
            data = json.loads(text)
        except json.JSONDecodeError:
            start = text.find("{")
            end = text.rfind("}")
            if start >= 0 and end > start:
                data = json.loads(text[start : end + 1])
            else:
                raise SearchClientError("Search model did not return valid JSON results") from None
        raw_items = data.get("results") if isinstance(data, dict) else data
        if not isinstance(raw_items, list):
            raise SearchClientError("Search model JSON missing results list")
        results: list[dict[str, str]] = []
        for item in raw_items:
            if not isinstance(item, dict):
                continue
            results.append(
                {
                    "title": str(item.get("title") or "").strip(),
                    "url": str(item.get("url") or "").strip(),
                    "snippet": str(item.get("snippet") or item.get("content") or "").strip()[:500],
                }
            )
        return results

    @classmethod
    def _parse_duckduckgo_html(cls, html_text: str, max_results: int) -> list[dict[str, str]]:
        links = SEARCH_RESULT_LINK_PATTERN.findall(html_text)
        snippets = SEARCH_RESULT_SNIPPET_PATTERN.findall(html_text)
        cleaned_snippets = [cls._clean_search_html(first or second) for first, second in snippets]
        results: list[dict[str, str]] = []
        for index, (href, title_html) in enumerate(links[:max_results]):
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
        if "uddg" in parse_qs(parsed.query):
            return unquote(parse_qs(parsed.query)["uddg"][0])
        return url

    @staticmethod
    def _clean_search_html(value: str) -> str:
        text = re.sub(r"<[^>]+>", " ", value or "")
        text = html.unescape(text)
        return re.sub(r"\s+", " ", text).strip()


def public_search_error(error: Exception) -> str:
    return safe_error_message(error)
