import type { AnalysisResultNode } from '../../../lib/ipc/analysisResults';

/** One analyzer run of the evidence and its record types. */
export interface AnalysisResultGroup {
  id: string;
  label: string;
  /** The result category's name. */
  category: string;
  /** What the run looked at (system or file); empty for the whole evidence. */
  target: string;
  /** The run node (its audit); results without one are grouped by analyzer. */
  run: AnalysisResultNode | null;
  warningCount: number;
  legacy: boolean;
  results: AnalysisResultNode[];
}

/** Names of tree nodes in the interface language (plugin translations). */
export interface AnalysisNodeLabels {
  node: (node: AnalysisResultNode) => string;
  category: (node: AnalysisResultNode) => string;
}

export const declaredNodeLabels: AnalysisNodeLabels = {
  node: node => node.label,
  category: node => node.typeName,
};

/** Flatten the evidence and category levels; each run keeps its record
 *  types, and a run with only warnings still appears. */
export function buildAnalysisGroups(
  tree: AnalysisResultNode[],
  imageId: string,
  query = '',
  labels: AnalysisNodeLabels = declaredNodeLabels,
): AnalysisResultGroup[] {
  const groups = new Map<string, AnalysisResultGroup>();
  const search = query.trim().toLocaleLowerCase();
  const matches = (text: string) => !search || text.toLocaleLowerCase().includes(search);
  function groupOf(owner: AnalysisResultNode | undefined, sample: AnalysisResultNode): AnalysisResultGroup {
    const id = owner?.nodeId ?? `${imageId}::${sample.function || sample.analyzer}`;
    let group = groups.get(id);
    if (!group) {
      const head = owner ?? sample;
      group = {
        id,
        label: owner ? labels.node(owner) : sample.analyzer,
        category: labels.category(head),
        target: head.targetLabel,
        run: owner ?? null,
        warningCount: owner?.warningCount ?? 0,
        legacy: head.legacy,
        results: [],
      };
      groups.set(id, group);
    }
    return group;
  }
  function visit(nodes: AnalysisResultNode[], owner?: AnalysisResultNode) {
    for (const node of nodes) {
      if (node.imageId !== imageId) continue;
      if (node.kind === 'function') {
        const header = groupOf(node, node);
        const headerMatches = matches(`${header.category} ${header.label} ${header.target}`);
        for (const result of node.children) {
          if (result.kind === 'result' && (headerMatches || matches(labels.node(result)))) header.results.push(result);
        }
        if (!header.results.length && !(headerMatches && node.children.length === 0)) groups.delete(header.id);
      } else if (node.kind === 'result') {
        const group = groupOf(owner, node);
        if (matches(`${group.category} ${group.label} ${labels.node(node)}`)) group.results.push(node);
        if (!group.results.length) groups.delete(group.id);
      } else {
        visit(node.children, owner);
      }
    }
  }
  visit(tree);
  return [...groups.values()];
}
