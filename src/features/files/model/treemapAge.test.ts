import { describe, expect, it } from 'vitest';
import { ageBucketOf, formatUnixSeconds } from './treemapAge';

describe('treemap ages', () => {
  const day = 86_400;
  it('measures age from the volume newest write', () => {
    const now = 1_767_225_600;
    expect(ageBucketOf(now - 2 * day, now)).toBe(0);
    expect(ageBucketOf(now - 20 * day, now)).toBe(1);
    expect(ageBucketOf(now - 100 * day, now)).toBe(2);
    expect(ageBucketOf(now - 300 * day, now)).toBe(3);
    expect(ageBucketOf(now - 3000 * day, now)).toBe(4);
    expect(ageBucketOf(now + day, now)).toBe(0);
    expect(ageBucketOf(null, now)).toBeNull();
    expect(ageBucketOf(now, null)).toBeNull();
  });

  it('formats Unix seconds like the list', () => {
    expect(formatUnixSeconds(1_766_217_600)).toBe('2025-12-20 08:00:00');
    expect(formatUnixSeconds(null)).toBe('—');
  });
});
