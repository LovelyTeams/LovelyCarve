import { describe, expect, it } from 'vitest';

import { mergeCarveArtifacts } from './carveResults';
import type { CarveArtifact } from '../../../lib/ipc/carve';

function artifact(id: string, offset: number): CarveArtifact {
  return {
    id,
    name: `f${offset}.png`,
    format_id: 'png',
    extension: 'png',
    category: 'image',
    offset,
    length: 20,
    confidence: 99,
    region: '整盘',
    read_fault: false,
  };
}

describe('mergeCarveArtifacts', () => {
  it('deduplicates overlapping pages and keeps image-offset order', () => {
    expect(mergeCarveArtifacts(
      [artifact('two', 20), artifact('one', 10)],
      [artifact('two', 20), artifact('three', 30)],
    ).map(item => item.id)).toEqual(['one', 'two', 'three']);
    expect(mergeCarveArtifacts(
      [artifact('one', 10), artifact('two', 20)],
      [artifact('two', 20), artifact('three', 30)],
    ).map(item => item.id)).toEqual(['one', 'two', 'three']);
  });

  it('appends a page that continues after the loaded tail', () => {
    const loaded = [artifact('one', 10), artifact('two', 20)];
    const page = [artifact('three', 30), artifact('four', 40)];
    expect(mergeCarveArtifacts(loaded, page)).toEqual([...loaded, ...page]);
    expect(mergeCarveArtifacts(loaded, [artifact('early', 5)]).map(item => item.id)).toEqual(['early', 'one', 'two']);
  });
});
