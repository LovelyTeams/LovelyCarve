interface Version {
  numbers: string[];
  prerelease: string[];
}

function parseVersion(tag: string): Version | null {
  const match = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([\da-zA-Z-]+(?:\.[\da-zA-Z-]+)*))?(?:\+[\da-zA-Z-]+(?:\.[\da-zA-Z-]+)*)?$/.exec(tag);
  if (!match) return null;
  const prerelease = match[4]?.split('.') ?? [];
  if (prerelease.some(part => /^\d+$/.test(part) && part.length > 1 && part.startsWith('0'))) return null;
  return { numbers: match.slice(1, 4), prerelease };
}

function compareNumber(left: string, right: string): number {
  return left.length - right.length || (left < right ? -1 : left > right ? 1 : 0);
}

function compareVersions(left: Version, right: Version): number {
  for (let index = 0; index < 3; index++) {
    const difference = compareNumber(left.numbers[index]!, right.numbers[index]!);
    if (difference) return difference;
  }
  if (!left.prerelease.length || !right.prerelease.length) {
    return Number(!left.prerelease.length) - Number(!right.prerelease.length);
  }
  for (let index = 0; index < Math.max(left.prerelease.length, right.prerelease.length); index++) {
    const leftPart = left.prerelease[index];
    const rightPart = right.prerelease[index];
    if (leftPart === undefined) return -1;
    if (rightPart === undefined) return 1;
    if (leftPart === rightPart) continue;
    const leftNumeric = /^\d+$/.test(leftPart);
    const rightNumeric = /^\d+$/.test(rightPart);
    if (leftNumeric && rightNumeric) return compareNumber(leftPart, rightPart);
    if (leftNumeric !== rightNumeric) return leftNumeric ? -1 : 1;
    return leftPart < rightPart ? -1 : 1;
  }
  return 0;
}

/** Compare semantic versions independently of GitHub's tag order. Stable builds ignore prereleases. */
export function findLatestVersionTag(tags: readonly string[], currentVersion: string): string | null {
  const current = parseVersion(currentVersion);
  if (!current) throw new Error('Invalid application version');
  let latest: { tag: string; version: Version } | null = null;
  for (const tag of tags) {
    const version = parseVersion(tag);
    if (!version || (!current.prerelease.length && version.prerelease.length)) continue;
    if (!latest || compareVersions(version, latest.version) > 0) latest = { tag, version };
  }
  return latest && compareVersions(latest.version, current) > 0 ? latest.tag : null;
}
