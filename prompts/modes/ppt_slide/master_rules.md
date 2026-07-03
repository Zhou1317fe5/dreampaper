You are extracting and applying a professional PowerPoint master style from a single uploaded template image.

Master extraction requirements:
- Treat the uploaded image as the highest-priority source for global slide style.
- Output the extracted master as `master_style_spec` so later page prompts can reuse the same structured global style.
- Extract immutable master elements separately from page-level body content.
- Record the same title region, page number, logo, corner marks, divider lines, safe margins, background, palette, typography hierarchy, module border/card style, and repeated decorative elements.
- Use approximate geometry in percentages or normalized coordinates when exact pixels are unavailable.
- Distinguish absent elements from uncertain elements. If no logo or page number is visible, say `not visible` and preserve the corresponding reserved region only when implied.

Required master fields:
- `canvas`: aspect ratio, orientation, output intent, and background treatment.
- `title_region`: location, width/height, alignment, title/subtitle hierarchy, and reserved whitespace.
- `safe_margins`: top/right/bottom/left content boundaries and forbidden overflow zones.
- `header_footer`: page number, logo, corner marks, footer/header behavior, and reserved regions.
- `divider_lines`: position, stroke style, color, and repeated separators.
- `palette`: background, primary, secondary, accent, neutral, and forbidden color drift.
- `typography`: title, subtitle, body, caption, numeric emphasis, font weight, size range, and alignment.
- `module_style`: card/frame border, radius, fill, shadow, separators, icon treatment, and whitespace rhythm.
- `decorative_elements`: repeated shapes, marks, bands, grids, or subtle ornaments.
- `immutable_elements`: elements that must appear in every generated slide with the same geometry and style.
- `page_layout_rules`: how variable A/B/C body skeletons may fit inside the safe body area without breaking the master.
- `forbidden_deviations`: style changes that would break deck consistency.

Application requirements for every slide page:
- The body layout may vary clearly across Template A, B, and C skeletons.
- Body variation must stay inside the extracted body safe area and must not move or restyle immutable master elements.
- Each page implement prompt must explicitly restate the master constraints: title region, page number/logo/corner marks, divider lines, safe margins, background, palette, typography, and module border/card style.
- Do not add extra page numbers, random logos, new corner marks, unrelated footer citations, gradients, stickers, emojis, cartoon decorations, or color palettes not found in the template.