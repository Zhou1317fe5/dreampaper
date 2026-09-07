import { describe, expect, it } from 'vitest';
import { classifyChar, judgeLine, OCR_POLICY } from './filter';

describe('judgeLine', () => {
  it('accepts ordinary Chinese, English, digits and punctuation', () => {
    expect(judgeLine('准确率 95.2%', 0.95)).toEqual({ accept: true, text: '准确率 95.2%' });
    expect(judgeLine('Model V2', 0.9)).toEqual({ accept: true, text: 'Model V2' });
    expect(judgeLine('样本数：100', 0.88)).toEqual({ accept: true, text: '样本数：100' });
    expect(judgeLine('樣本數（繁體），OK!', 0.9).accept).toBe(true);
    expect(judgeLine('  v1.2.3-beta/final  ', 0.9)).toEqual({ accept: true, text: 'v1.2.3-beta/final' });
  });

  it('rejects formula-class content regardless of score', () => {
    for (const line of ['∑x²', 'α+β=γ', 'x ≤ 3', '√2 ≈ 1.41', '1920×1080', 'H₂O', 'E = mc²', '→ next']) {
      expect(judgeLine(line, 0.99)).toEqual({ accept: false, reason: 'unsupported' });
    }
  });

  it('rejects low confidence and empty lines', () => {
    expect(judgeLine('准确率', OCR_POLICY.minScore - 0.01)).toEqual({ accept: false, reason: 'low_confidence' });
    expect(judgeLine('   ', 1)).toEqual({ accept: false, reason: 'empty' });
    expect(judgeLine('...', 1)).toEqual({ accept: false, reason: 'unsupported' });
  });

  it('classifies characters', () => {
    expect(classifyChar('中')).toBe('text');
    expect(classifyChar('Z')).toBe('text');
    expect(classifyChar('，')).toBe('punct');
    expect(classifyChar(' ')).toBe('space');
    expect(classifyChar('β')).toBe('unsupported');
    expect(classifyChar('²')).toBe('unsupported');
  });
});
