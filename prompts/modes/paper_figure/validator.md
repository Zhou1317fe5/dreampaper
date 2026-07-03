Validate that the paper figure design response is strict JSON and follows the split Figure contract.

Common checks:
- Contains `figure` with `visual_type`, `layout_constraints`, `semantic_constraints`, `visual_constraints`, `forbidden_errors`, `quality_rubric`, `visible_text`, and a complete `implement_prompt`.
- `implement_prompt` is long enough to guide direct image generation and explicitly covers publication quality, faithfulness, conciseness, readability, forbidden errors, and template boundary.
- Visible labels are short, meaningful, and free of gibberish or fake formulas.
- The prompt never requests direct template copying, tracing, editing, duplication, or background editing.

Diagram checks:
- `diagram_spec` contains modules, entities, connections, flow direction, grouping hierarchy, arrow routing, and label strategy.
- Connections preserve source/target direction and avoid hallucinated modules or reversed flows.

Plot/chart checks:
- `plot_spec` contains chart type, data fields, axes, units, series/categories, legend, statistical annotations, and data integrity rules.
- Plot prompts forbid data distortion, wrong chart type, fabricated labels, unsupported statistics, and unreadable data elements.