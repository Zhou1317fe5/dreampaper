import asyncio
import json
import tempfile
import unittest
from pathlib import Path
from typing import Any

from backend.app.config import ConfigStore
from backend.app.jobs import DesignSchemaError, JobManager
from backend.app.prompts import PromptStore, compose_prompt


ROOT = Path(__file__).resolve().parents[1]
PROMPT_ROOT = ROOT / "prompts"


def long_figure_prompt(extra: str = "") -> str:
    return (
        "Create a publication quality academic paper figure with faithful grounded content and no hallucination. "
        "Use concise visual abstraction, short labels, readable legible typography, high contrast, and a clean white background. "
        "Use selected templates as reference style and layout inspiration only, never as editable base imagery. "
        "Avoid scope drift, gibberish, fake formulas, unreadable arrows, visual noise, and unsupported claims. "
        "Keep all modules and data encodings aligned with the user-provided section. "
        f"{extra}"
    )


def valid_diagram_design() -> dict[str, Any]:
    return {
        "figure": {
            "title": "Pipeline overview",
            "visual_type": "diagram",
            "aspect_ratio": "16:9",
            "template_usage": "balanced",
            "layout_constraints": ["16:9 compact rectangular canvas", "left-to-right module rhythm"],
            "semantic_constraints": ["all modules grounded in user method", "preserve flow direction"],
            "visual_constraints": ["high contrast", "publication friendly white background"],
            "forbidden_errors": ["hallucination", "reversed flow", "scope violation", "text overload"],
            "quality_rubric": {
                "faithfulness": "ground every block in source text",
                "conciseness": "use visual abstraction and keywords",
                "readability": "legible labels and clear arrows",
                "aesthetics": "restrained academic style",
            },
            "visible_text": ["输入", "编码器", "输出"],
            "diagram_spec": {
                "modules": ["Input", "Encoder", "Output"],
                "entities": ["data", "features"],
                "connections": [{"source": "Input", "target": "Encoder", "meaning": "feature extraction"}],
                "flow_direction": "left to right",
                "grouping_hierarchy": "three top-level modules",
                "arrow_routing": "straight arrows with minimal crossings",
                "label_strategy": "short noun labels only",
            },
            "implement_prompt": long_figure_prompt("Render three rounded modules connected by clean arrows and compact grouping."),
        },
        "quality_checklist": ["faithful", "concise", "readable"],
    }


def valid_plot_design() -> dict[str, Any]:
    return {
        "figure": {
            "title": "Ablation chart",
            "visual_type": "plot",
            "aspect_ratio": "16:9",
            "template_usage": "balanced",
            "layout_constraints": ["single axes plot", "legend outside data region"],
            "semantic_constraints": ["represent only supplied metrics", "no fabricated statistics"],
            "visual_constraints": ["colorblind-friendly series", "readable axis labels"],
            "forbidden_errors": ["data distortion", "wrong chart type", "unsupported statistics"],
            "quality_rubric": {
                "faithfulness": "accurate values and labels",
                "conciseness": "only necessary chart elements",
                "readability": "clear axes and legend",
                "aesthetics": "publication-ready plot",
            },
            "visible_text": ["准确率", "方法A", "方法B"],
            "plot_spec": {
                "chart_type": "grouped bar chart",
                "data_fields": ["method", "accuracy"],
                "axes": {"x": "方法", "y": "准确率 (%)"},
                "units": "percent",
                "series_or_categories": ["方法A", "方法B"],
                "legend": "top-right outside data area",
                "statistical_annotations": "none",
                "data_integrity_rules": "No value distortion, misleading scales, label fabrication, wrong chart type, or unsupported statistical marks.",
            },
            "implement_prompt": long_figure_prompt(
                "Draw accurate axes, units, ticks, legend, grouped bars, and data labels without unsupported p-values."
            ),
        },
        "quality_checklist": ["faithful", "concise", "readable"],
    }


def valid_template_analysis() -> dict[str, Any]:
    return {
        "template_analysis": {
            "master_style_summary": "Red-gray academic master with fixed title band and footer page marker.",
            "master_style_spec": {
                "canvas": {"aspect_ratio": "16:9", "orientation": "landscape", "background": "pure white"},
                "title_region": {"position": "x=6%, y=5%, w=70%, h=10%", "alignment": "left", "hierarchy": "bold title", "reserved_whitespace": "top band"},
                "safe_margins": {"top": "18%", "right": "7%", "bottom": "10%", "left": "7%", "body_area": "central rectangle"},
                "header_footer": {"page_number": "bottom right", "logo": "top right", "corner_marks": "red corner", "reserved_regions": ["top-right logo", "bottom-right page"]},
                "divider_lines": [{"position": "under title", "stroke": "2px", "color": "#B40000"}],
                "palette": {"background": "#FFFFFF", "primary": "#B40000", "secondary": "#555555", "accent": "#D8D8D8", "neutral": "#222222", "forbidden_drift": "no blue/purple"},
                "typography": {"title": "32px bold", "subtitle": "20px", "body": "18px", "caption": "14px", "alignment": "left"},
                "module_style": {"border": "1px solid #D8D8D8", "radius": "10px", "fill": "#FFFFFF", "shadow": "none", "spacing": "24px"},
                "decorative_elements": ["red divider", "gray card borders"],
                "immutable_elements": ["title region", "page number/logo/corner marks", "divider lines", "background", "palette", "typography", "module/card style"],
                "page_layout_rules": "A/B/C body skeletons stay inside central safe area.",
                "forbidden_deviations": ["moving title", "palette drift", "new logo", "overflowing safe margins"],
            },
            "global_constraints": ["keep title band", "keep footer marker", "keep palette"],
            "immutable_elements": ["title region", "page number", "divider lines", "palette"],
            "page_layout_rules": "Body can vary but must stay inside safe margins.",
        }
    }


def long_slide_prompt() -> str:
    return (
        "Create one 16:9 academic PowerPoint-style slide. Use Template B with three academic cards in the body safe area. "
        "Card one visible text 数据处理, card two visible text 模型训练, card three visible text 结果验证. "
        "Use a small process diagram with arrows between cards and keep all body content inside the safe body area."
    )


def valid_pages() -> dict[str, Any]:
    return {
        "pages": [
            {
                "page": 1,
                "selected_template": "Template B",
                "title": "研究路线",
                "slide_type": "technical body",
                "body_layout_plan": "Three cards inside central safe body area.",
                "master_style_binding": {
                    "title_region": "same extracted title region and hierarchy",
                    "safe_margins": "same body safe area",
                    "header_footer": "same page number logo corner mark",
                    "divider_lines": "same red divider line",
                    "palette": "same white red gray color palette",
                    "typography": "same font hierarchy and sizes",
                    "module_style": "same card border radius fill shadow",
                    "background": "same white background",
                },
                "visual_element_plan": {
                    "usage_decision": "Use one semantic workflow icon and one tool-like mark because the page explains a technical route.",
                    "elements": [
                        {
                            "type": "icon",
                            "subject": "数据处理流程",
                            "source_reference": "generic semantic icon",
                            "placement": "left side of each body card header",
                            "style": "thin line icon in template crimson and gray palette",
                            "size_ratio": "small, about 6% of body area per icon",
                        }
                    ],
                    "text_visual_balance": "Icons stay small and support three concise text cards without replacing content.",
                },
                "emphasis_plan": {
                    "keywords": [
                        {"text": "数据处理", "style": "bold", "reason": "first workflow step"},
                        {"text": "模型训练", "style": "template-primary-red", "reason": "core technical action"},
                    ],
                    "style_rules": "Highlight only two short phrases using bold or template crimson.",
                },
                "visible_text": ["研究路线", "数据处理", "模型训练"],
                "implement_prompt": long_slide_prompt(),
            }
        ]
    }


def valid_outline(page_count: int = 2) -> dict[str, Any]:
    briefs = []
    for page in range(1, page_count + 1):
        briefs.append(
            {
                "page": page,
                "title": f"第{page}页标题",
                "role": "technical body" if page > 1 else "problem",
                "main_message": f"第{page}页核心信息",
                "content_points": ["要点一", "要点二", "要点三"],
                "suggested_template": "Template B",
                "visual_direction": "Use one semantic icon and one compact diagram.",
                "transition_from_previous": "承接上一页" if page > 1 else "none",
                "transition_to_next": "引出下一页" if page < page_count else "closure",
            }
        )
    return {
        "deck_outline": {
            "deck_title": "测试演示文稿",
            "deck_goal": "说明核心流程",
            "narrative_arc": "从问题到方法再到总结",
            "shared_prompt": {
                "audience": "academic users",
                "tone": "academic",
                "global_style_constraints": "Use extracted master style as immutable.",
                "terminology": ["流程", "方法"],
                "visual_language": "Use restrained icons.",
                "emphasis_language": "Highlight 2-5 short phrases only.",
            },
            "page_briefs": briefs,
        }
    }


class FakeDesignClient:
    def __init__(self, response: dict[str, Any]) -> None:
        self.response = response
        self.calls = 0
        self.last_prompt = ""

    async def generate(self, *args: Any, **kwargs: Any) -> str:
        self.calls += 1
        self.last_prompt = args[2]
        return json.dumps(self.response, ensure_ascii=False)


class FakeTextDesignClient:
    def __init__(self, responses: list[str]) -> None:
        self.responses = responses
        self.calls = 0
        self.prompts: list[str] = []

    async def generate(self, *args: Any, **kwargs: Any) -> str:
        self.prompts.append(args[2])
        response = self.responses[min(self.calls, len(self.responses) - 1)]
        self.calls += 1
        return response


class PromptContractTests(unittest.TestCase):
    def test_prompt_assets_load_and_compose(self) -> None:
        store = PromptStore(PROMPT_ROOT)
        keys = [
            "modes/paper_figure/diagram_rules.md",
            "modes/paper_figure/plot_rules.md",
            "modes/ppt_slide/master_rules.md",
        ]
        assets = [store.load(key) for key in keys]
        prompt, metadata = compose_prompt(assets, {"Output Contract": "{}"})
        self.assertIn("PaperBanana", prompt)
        self.assertIn("master_style_spec", prompt)
        self.assertEqual([item["key"] for item in metadata], keys)
        self.assertTrue(all(item["hash"] and item["version"] == item["hash"] for item in metadata))

    def test_contracts_expose_strengthened_fields(self) -> None:
        paper = JobManager._paper_contract()
        template = JobManager._template_analysis_contract()
        pages = JobManager._ppt_pages_contract(2, {"aspect_ratio": "16:9", "image_size": "4K"})
        outline = JobManager._ppt_outline_contract(2)
        single_page = JobManager._ppt_single_page_contract(1)
        for token in ("diagram_spec", "plot_spec", "quality_rubric", "data_integrity_rules"):
            self.assertIn(token, paper)
        for token in ("master_style_spec", "immutable_elements", "forbidden_deviations"):
            self.assertIn(token, template)
        for token in ("master_style_binding", "safe_margins", "module_style", "visual_element_plan", "emphasis_plan"):
            self.assertIn(token, pages)
            self.assertIn(token, single_page)
        for token in ("deck_outline", "shared_prompt", "page_briefs"):
            self.assertIn(token, outline)
        self.assertIn("Do not write page-level implement_prompt", outline)
        self.assertNotIn('"implement_prompt":', outline)
        self.assertNotIn("size=", pages)
        self.assertIn("Do not put API output parameters", pages)

    def test_validates_diagram_and_plot_contracts(self) -> None:
        JobManager._validate_paper_design(valid_diagram_design())
        JobManager._validate_paper_design(valid_plot_design())
        invalid = valid_diagram_design()
        del invalid["figure"]["diagram_spec"]
        with self.assertRaises(DesignSchemaError):
            JobManager._validate_paper_design(invalid)
        copy_request = valid_diagram_design()
        copy_request["figure"]["implement_prompt"] = long_figure_prompt("Copy the template exactly as a base image.")
        with self.assertRaises(ValueError):
            JobManager._validate_paper_design(copy_request)

    def test_validates_template_analysis_and_pages(self) -> None:
        template_analysis = valid_template_analysis()
        outline = JobManager._validate_ppt_outline(valid_outline(2), 2)
        self.assertEqual([brief["page"] for brief in outline["page_briefs"]], [1, 2])
        page = JobManager._validate_ppt_single_page({"page": valid_pages()["pages"][0]}, 1, template_analysis)
        self.assertEqual(page["page"], 1)
        broken_outline = valid_outline(2)
        del broken_outline["deck_outline"]["page_briefs"][0]["content_points"]
        with self.assertRaises(DesignSchemaError):
            JobManager._validate_ppt_outline(broken_outline, 2)
        wrong_page = {"page": {**valid_pages()["pages"][0], "page": 2}}
        with self.assertRaises(ValueError):
            JobManager._validate_ppt_single_page(wrong_page, 1, template_analysis)
        pages = valid_pages()
        JobManager._validate_template_analysis(template_analysis)
        validated = JobManager._validate_ppt_pages(pages, 1, template_analysis)
        self.assertEqual(validated[0]["page"], 1)
        broken = valid_pages()
        del broken["pages"][0]["master_style_binding"]["divider_lines"]
        with self.assertRaises(DesignSchemaError):
            JobManager._validate_ppt_pages(broken, 1, template_analysis)
        missing_visual = valid_pages()
        del missing_visual["pages"][0]["visual_element_plan"]
        with self.assertRaises(DesignSchemaError):
            JobManager._validate_ppt_pages(missing_visual, 1, template_analysis)
        too_much_emphasis = valid_pages()
        too_much_emphasis["pages"][0]["emphasis_plan"]["keywords"] = [
            {"text": f"短语{i}", "style": "bold", "reason": "test"} for i in range(6)
        ]
        with self.assertRaises(DesignSchemaError):
            JobManager._validate_ppt_pages(too_much_emphasis, 1, template_analysis)
        with_api_fields = valid_pages()
        with_api_fields["pages"][0]["implement_prompt"] += " size=2048x1152, quality=auto, image_size=4K."
        with self.assertRaises(DesignSchemaError):
            JobManager._validate_ppt_pages(with_api_fields, 1, template_analysis)

    def test_ppt_master_prefix_is_uniform_and_strips_api_settings(self) -> None:
        template_analysis = valid_template_analysis()
        pages = valid_pages()["pages"]
        second_page = {**pages[0], "page": 2, "title": "第二页", "implement_prompt": "Create one 16:9 slide, size=2048x1152, image_size=4K. Show a timeline."}
        merged = JobManager._apply_ppt_master_prompt_prefix([pages[0], second_page], template_analysis)
        first_prefix = merged[0]["implement_prompt"].split("\n\nPage-specific body layout and content:\n", 1)[0]
        second_prefix = merged[1]["implement_prompt"].split("\n\nPage-specific body layout and content:\n", 1)[0]
        self.assertEqual(first_prefix, second_prefix)
        for item in merged:
            self.assertIn("Use the extracted template master specification below as immutable", item["implement_prompt"])
            self.assertNotIn("size=", item["implement_prompt"])
            self.assertNotIn("image_size=", item["implement_prompt"])
            self.assertIn("Create one 16:9 academic PowerPoint-style slide", item["implement_prompt"])
            self.assertIn("Visual element plan", item["implement_prompt"])
            self.assertIn("Keyword emphasis plan", item["implement_prompt"])
            self.assertIn("Highlight only the planned key phrases", item["implement_prompt"])

    def test_visual_asset_context_helpers_are_text_only(self) -> None:
        terms = JobManager._extract_visual_asset_terms("本方案使用 Docker、Kubernetes 和 OpenAI API 构建部署流程。")
        self.assertIn("Docker", terms)
        self.assertIn("Kubernetes", terms)
        html_text = '''
        <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.docker.com%2Fcompany%2Fnewsroom%2Fmedia-resources%2F">Docker Media Resources</a>
        <a class="result__snippet">Official Docker logos and brand resources.</a>
        '''
        results = JobManager._parse_visual_asset_search_results(html_text)
        self.assertEqual(results[0]["url"], "https://www.docker.com/company/newsroom/media-resources/")
        context_text = JobManager._visual_asset_context_text(
            {
                "terms": ["Docker"],
                "items": [{"term": "Docker", "query": "Docker official logo product render visual appearance", "results": results}],
            }
        )
        self.assertIn("text-only evidence", context_text)
        self.assertIn("Docker Media Resources", context_text)
        self.assertNotIn("base64", context_text.lower())

    def test_ppt_page_planner_context_is_compacted(self) -> None:
        full = json.dumps(valid_template_analysis(), ensure_ascii=False, indent=2)
        compact = JobManager._compact_template_analysis(valid_template_analysis())
        self.assertLess(len(compact), len(full))
        self.assertLessEqual(len(compact), 2600)
        long_text = "材料" * 3000
        truncated = JobManager._truncate_text(long_text, 100)
        self.assertIn("Truncated from", truncated)
        self.assertLess(len(truncated), len(long_text))

    def test_ordered_batches_queue_after_concurrency_limit(self) -> None:
        manager = JobManager(None, None, None, None)  # type: ignore[arg-type]
        running = 0
        max_running = 0
        started: list[int] = []

        async def worker(item: int) -> int:
            nonlocal running, max_running
            started.append(item)
            running += 1
            max_running = max(max_running, running)
            await asyncio.sleep(0.001)
            running -= 1
            return item * 10

        results = asyncio.run(manager._run_in_ordered_batches([1, 2, 3, 4, 5], 2, worker))
        self.assertEqual(results, [10, 20, 30, 40, 50])
        self.assertLessEqual(max_running, 2)
        self.assertEqual(started[:2], [1, 2])
        self.assertEqual(started[2:4], [3, 4])

    def test_ppt_concurrency_defaults_to_page_count_and_config_persists(self) -> None:
        self.assertEqual(JobManager._resolve_ppt_concurrency(None, 5), 5)
        self.assertEqual(JobManager._resolve_ppt_concurrency(3, 5), 3)
        self.assertEqual(JobManager._resolve_ppt_concurrency(10, 5), 5)

        with tempfile.TemporaryDirectory() as tmpdir:
            store = ConfigStore(Path(tmpdir) / "config.json")
            config = store.load().model_copy(update={"ppt_page_plan_concurrency": 4, "ppt_image_concurrency": 2})
            store.save(config)
            public = store.public()

        self.assertEqual(public.ppt_page_plan_concurrency, 4)
        self.assertEqual(public.ppt_image_concurrency, 2)

    def test_json_parse_failure_retries_same_context_before_repair(self) -> None:
        manager = JobManager(None, None, None, None)  # type: ignore[arg-type]
        fake_design = FakeTextDesignClient([json.dumps({"ok": True})])
        manager.design = fake_design  # type: ignore[assignment]

        parsed = asyncio.run(
            manager._parse_or_repair(
                profile=object(),
                system_prompt="system",
                original_prompt="original task with complete context",
                text='{"ok": true "broken": false}',
                images=[],
            )
        )

        self.assertEqual(parsed, {"ok": True})
        self.assertEqual(fake_design.calls, 1)
        self.assertIn("original task with complete context", fake_design.prompts[0])
        self.assertIn("Regenerate the answer using the same task context", fake_design.prompts[0])
        self.assertIn("Previous invalid output excerpt", fake_design.prompts[0])

    def test_structured_fill_retry_only_fills_schema_gaps(self) -> None:
        manager = JobManager(None, None, None, None)  # type: ignore[arg-type]
        filled = valid_diagram_design()
        fake_design = FakeDesignClient(filled)
        manager.design = fake_design  # type: ignore[assignment]
        initial = valid_diagram_design()
        del initial["figure"]["diagram_spec"]

        parsed, _, retry = asyncio.run(
            manager._parse_validate_or_fill_missing(
                profile=object(),
                system_prompt="system",
                original_prompt="original task",
                text=json.dumps(initial, ensure_ascii=False),
                images=[],
                validator=JobManager._validate_paper_design,
            )
        )

        self.assertEqual(parsed, filled)
        self.assertTrue(retry["attempted"])
        self.assertEqual(fake_design.calls, 1)
        self.assertIn("Only fill", fake_design.last_prompt)
        self.assertIn("Diagram figure missing diagram_spec", fake_design.last_prompt)


if __name__ == "__main__":
    unittest.main()
