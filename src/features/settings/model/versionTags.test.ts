import { describe, expect, it } from 'vitest';
import { findLatestVersionTag } from './versionTags';

describe('version tags', () => {
  it('uses numeric version precedence rather than tag order, string order or build metadata', () => {
    expect(findLatestVersionTag(['v1.2.1', 'v1.10.0', 'v1.9.9', '1.10.0+build.2'], '1.2.0')).toBe('v1.10.0');
    expect(findLatestVersionTag(['v1.2.0+new', 'v1.1.9'], '1.2.0+old')).toBeNull();
    expect(findLatestVersionTag(['v1.2.0'], '1.3.0')).toBeNull();
  });

  it('ignores prereleases for stable builds and unrelated or malformed tags', () => {
    expect(findLatestVersionTag(['nightly', 'v2.0.0-beta.1', 'v01.3.0', '1.3', '1.3.0-01', 'release/3.0.0'], '1.2.0')).toBeNull();
    expect(findLatestVersionTag([], '1.2.0')).toBeNull();
  });

  it('orders prerelease identifiers by SemVer rules, including promotion to a stable build', () => {
    const ordered = ['1.0.0-alpha', '1.0.0-alpha.1', '1.0.0-alpha.beta', '1.0.0-beta', '1.0.0-beta.2', '1.0.0-beta.11', '1.0.0-rc.1', '1.0.0'];
    for (let index = 1; index < ordered.length; index++) {
      expect(findLatestVersionTag(ordered.slice(0, index + 1).reverse(), ordered[index - 1]!)).toBe(ordered[index]);
    }
    expect(findLatestVersionTag(['2.0.0-beta.10', '2.0.0-beta.9'], '2.0.0-beta.2')).toBe('2.0.0-beta.10');
  });

  it('compares large numeric segments without losing integer precision', () => {
    expect(findLatestVersionTag(['1.9007199254740993.0', '1.9007199254740992.0'], '1.2.0')).toBe('1.9007199254740993.0');
    expect(() => findLatestVersionTag([], 'development')).toThrow('Invalid application version');
  });
});
