import type { CarveArtifact } from '../../../lib/ipc/carve';

function compareArtifacts(left: CarveArtifact, right: CarveArtifact): number {
  return left.offset - right.offset || left.id.localeCompare(right.id);
}

/** Merge incrementally paged carve findings without duplicating event races. */
export function mergeCarveArtifacts(
  current: readonly CarveArtifact[],
  incoming: readonly CarveArtifact[],
): CarveArtifact[] {
  if (incoming.length === 0) return [...current];
  // Pages normally continue strictly after the loaded tail; appending then
  // cannot duplicate an id and keeps a full drain linear.
  const ascending = (list: readonly CarveArtifact[]) => list.every((artifact, index) => index === 0 || compareArtifacts(list[index - 1]!, artifact) < 0);
  const tail = current[current.length - 1];
  if ((!tail || compareArtifacts(tail, incoming[0]!) < 0) && ascending(incoming) && ascending(current)) return [...current, ...incoming];
  const byId = new Map(current.map(artifact => [artifact.id, artifact]));
  for (const artifact of incoming) byId.set(artifact.id, artifact);
  return [...byId.values()].sort(compareArtifacts);
}
