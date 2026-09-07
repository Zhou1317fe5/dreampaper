// OCR text policy: which recognised lines may become editable text.
//
// The model's dictionary covers formula symbols and Greek letters, so the
// product filters after recognition. Precision wins over recall: a line with
// any formula-class character, or below the confidence threshold, only leaves
// a background fill behind and the user types the text by hand.

export const OCR_POLICY = {
  version: 1,
  /** Lines scoring below this are treated as "no reliable text". */
  minScore: 0.8
} as const;

export type Verdict =
  | { accept: true; text: string }
  | { accept: false; reason: 'empty' | 'low_confidence' | 'unsupported' };

const ALLOWED_PUNCTUATION = new Set(
  Array.from(
    ',.!?:;\'"()[]{}-_/%+&@#*=<>~`^|\\' +
      '，。！？：；、“”‘’（）【】《》〈〉—…·～％＋－「」『』〔〕・﹣－：'
  )
);

const UNSUPPORTED_RANGES: Array<[number, number]> = [
  [0x0370, 0x03ff], // Greek and Coptic
  [0x1d400, 0x1d7ff], // Mathematical alphanumerics
  [0x2070, 0x209f], // Super/subscripts
  [0x2100, 0x214f], // Letterlike symbols
  [0x2150, 0x218f], // Number forms (fractions)
  [0x2190, 0x21ff], // Arrows
  [0x2200, 0x22ff], // Mathematical operators
  [0x2300, 0x23ff], // Misc technical
  [0x25a0, 0x25ff], // Geometric shapes
  [0x27c0, 0x27ef], // Misc math symbols A
  [0x2980, 0x29ff], // Misc math symbols B
  [0x2a00, 0x2aff] // Supplemental math operators
];

const UNSUPPORTED_CHARS = new Set(Array.from('×÷±¬°µ¹²³¼½¾√∞∑∏∫∂≈≠≤≥'));

function isCjk(code: number): boolean {
  return (
    (code >= 0x4e00 && code <= 0x9fff) ||
    (code >= 0x3400 && code <= 0x4dbf) ||
    (code >= 0x20000 && code <= 0x2ebef) ||
    (code >= 0xf900 && code <= 0xfaff) ||
    (code >= 0x3040 && code <= 0x30ff)
  );
}

function isFullwidthAlnum(code: number): boolean {
  return (code >= 0xff10 && code <= 0xff19) || (code >= 0xff21 && code <= 0xff3a) || (code >= 0xff41 && code <= 0xff5a);
}

function isLatin(code: number): boolean {
  return (
    (code >= 0x41 && code <= 0x5a) ||
    (code >= 0x61 && code <= 0x7a) ||
    (code >= 0x30 && code <= 0x39) ||
    (code >= 0x00c0 && code <= 0x024f && code !== 0x00d7 && code !== 0x00f7)
  );
}

export function classifyChar(ch: string): 'text' | 'space' | 'punct' | 'unsupported' {
  const code = ch.codePointAt(0) ?? 0;
  if (/\s/.test(ch)) return 'space';
  if (UNSUPPORTED_CHARS.has(ch)) return 'unsupported';
  if (UNSUPPORTED_RANGES.some(([lo, hi]) => code >= lo && code <= hi)) return 'unsupported';
  if (ALLOWED_PUNCTUATION.has(ch)) return 'punct';
  if (isCjk(code) || isLatin(code) || isFullwidthAlnum(code)) return 'text';
  return 'unsupported';
}

/** Decide whether one recognised line becomes a text layer. */
export function judgeLine(text: string, score: number): Verdict {
  const trimmed = text.replace(/\s+/g, ' ').trim();
  if (!trimmed) return { accept: false, reason: 'empty' };
  let letters = 0;
  for (const ch of trimmed) {
    const kind = classifyChar(ch);
    if (kind === 'unsupported') return { accept: false, reason: 'unsupported' };
    if (kind === 'text') letters += 1;
  }
  if (letters === 0) return { accept: false, reason: 'unsupported' };
  if (!(score >= OCR_POLICY.minScore)) return { accept: false, reason: 'low_confidence' };
  return { accept: true, text: trimmed };
}
