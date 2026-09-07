import { describe, expect, it } from 'vitest';
import { desktopCopy } from './copy';
import { BACKGROUND_PRESETS, composeRules, FONT_CHOICES, normalizeHex, styleLines } from './style_picker';

const zh = desktopCopy.zh.style;
const en = desktopCopy.en.style;

describe('style constraints', () => {
  it('normalises hex colours and rejects anything else', () => {
    expect(normalizeHex('#ffffff')).toBe('#FFFFFF');
    expect(normalizeHex('1f2a44')).toBe('#1F2A44');
    expect(normalizeHex('#abc')).toBe('#AABBCC');
    expect(normalizeHex('#12345')).toBeNull();
    expect(normalizeHex('white')).toBeNull();
    expect(normalizeHex('')).toBeNull();
    for (const preset of BACKGROUND_PRESETS) expect(normalizeHex(preset)).toBe(preset);
  });

  it('adds nothing when nothing was chosen', () => {
    expect(styleLines({ font: '', background: '' }, zh)).toEqual([]);
    expect(composeRules('', { font: '', background: '' }, zh)).toBeNull();
    expect(composeRules('  ', { font: '', background: 'not-a-colour' }, zh)).toBeNull();
  });

  it('appends font and background sentences after the free-text rules', () => {
    const rules = composeRules('禁止出现公式', { font: FONT_CHOICES[0], background: '#eaf2fb' }, zh);
    expect(rules).toBe(`禁止出现公式\n${zh.fontLine(FONT_CHOICES[0])}\n${zh.backgroundLine('#EAF2FB')}`);
    expect(composeRules('', { font: 'Arial', background: '' }, en)).toBe(en.fontLine('Arial'));
  });
});
