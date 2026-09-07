Shared expression rules for the design stage. They apply to paper figures and to PPT deck outlines and single pages alike. Run the analysis procedure below before writing `implement_prompt`, and make the resulting constraints explicit inside `implement_prompt`.

Single carrier per information unit:
- An information unit is one thing the reader must take away: a data series, a comparison, a mechanism, a claim, a step sequence, a component, a metric.
- Every information unit appears exactly once, through exactly one carrier: table, chart, diagram, text, icon, or object depiction.
- No semantic duplication between graphic and graphic, text and text, or graphic and text.
- Anti-example: a table already lists per-item growth values and a bar chart of the same values sits beside it. That is duplicate expression; keep one carrier and drop the other.
- Do not restate in a text block what a diagram already shows. Do not caption an icon with a sentence that repeats the module label next to it. Do not draw a flowchart and also write a numbered text list of the same steps.
- When a unit could fit two carriers, choose the one that communicates fastest for that unit type: quantities go to a chart or table (never both), relations and flows go to a diagram, a single claim goes to text.

Core-first composition:
- Extract the core idea of the material first. Decide what the reader must see within the first second.
- The core content occupies the main body region and carries the visual center of gravity: largest area, strongest contrast, central or leading position in the reading order.
- Secondary content goes to the periphery, shrinks, or is dropped. Never tile all material evenly across the canvas as if every unit mattered equally.
- Paper figures: the core mechanism or the main pipeline route is the visual protagonist; auxiliary branches, training loops, and pre-processing bands are subordinate in size and position.
- Slides: the page `main_message` sits in the main body area; supporting points orbit it rather than compete with it.

Hierarchy consistency:
- Text at the same hierarchy level shares one font size, one weight, and one color everywhere on the canvas.
- Adjacent levels differ visibly in size or weight so the reader can tell them apart without effort.
- PPT pages must not exceed the typography levels defined by the template master. Paper figures use at most three levels: title, label, annotation.
- Emphasis is a style applied inside a level, not a new level.

Boundary alignment:
- Module and card outer edges align to a shared grid; text baselines inside a row align.
- Modules in the same row share one height; modules in the same column share one width.
- Gutters between modules are uniform. Nothing crosses the safe margins.
- Arrows and connectors leave and enter modules at consistent anchor points.

Required analysis procedure before writing `implement_prompt`:
1. List the information units found in the material.
2. Assign exactly one carrier to each unit and record why that carrier fits.
3. Run a redundancy pass: find every unit expressed twice and merge or remove the duplicate; record what was removed.
4. Mark the core unit or units and name the focus region where they will sit.
5. Define the hierarchy levels (font size, weight, color per level) and the alignment rules the layout follows.

These steps are recorded in the JSON fields `information_units` (one entry per unit with its carrier and reason), `redundancy_check` (what was merged or removed plus a statement confirming no graphic-graphic, text-text, or graphic-text duplication remains), and `hierarchy_plan` (levels, alignment rules, focus region).

`implement_prompt` must explicitly restate these four constraints so the implement model receives them directly: one carrier per information unit; uniform font size for same-level text; edges and baselines aligned to a shared grid; core content in the main body region.
