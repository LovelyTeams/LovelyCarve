import { sourceText, type Translator } from '../../../i18n/translate';
import type { AnalysisResultNode, BcpRowGroup } from '../../../lib/ipc/analysisResults';
import type { ImageId } from '../../../lib/ipc/ids';
import { bcpApplication } from './bcpApplications';

export const BCP_PLUGIN_ID = 'org.lovelycarve.bcp';

export interface BcpResultItem {
  node: AnalysisResultNode;
  deleted: number;
}

export interface BcpAccountDirectory {
  id: string;
  account: string;
  label: string;
  total: number;
  deleted: number;
  results: BcpResultItem[];
}

export interface BcpAppDirectory {
  id: string;
  label: string;
  icon: string;
  codes: string[];
  total: number;
  deleted: number;
  accounts: BcpAccountDirectory[];
  results: BcpResultItem[];
}

export interface BcpDirectory {
  run: AnalysisResultNode;
  evidenceLabel: string;
  apps: BcpAppDirectory[];
}

function buildDirectory(run: AnalysisResultNode, evidenceLabel: string, tr: Translator): BcpDirectory {
  const apps = new Map<string, BcpAppDirectory>();
  const accounts = new Map<string, BcpAccountDirectory>();
  const leaves = new Map<string, BcpResultItem>();
  for (const node of run.children) {
    const groups: BcpRowGroup[] = node.bcpGroups ?? [{ application: '', account: '', accountLabel: null, total: node.childCount, deleted: 0 }];
    for (const group of groups) {
      const appInfo = bcpApplication(group.identifiedApplication ?? group.application, tr, node.artifactType);
      let app = apps.get(appInfo.id);
      if (!app) {
        app = { ...appInfo, id: JSON.stringify([run.nodeId, appInfo.id]), codes: [], total: 0, deleted: 0, accounts: [], results: [] };
        apps.set(appInfo.id, app);
      }
      if (group.application && !app.codes.includes(group.application)) app.codes.push(group.application);
      let results = app.results;
      if (group.account) {
        const id = JSON.stringify([app.id, group.account]);
        let account = accounts.get(id);
        if (!account) {
          account = { id, account: group.account, label: group.account, total: 0, deleted: 0, results: [] };
          accounts.set(id, account);
          app.accounts.push(account);
        }
        if (group.accountLabel && account.label === account.account) account.label = group.accountLabel;
        account.total += group.total;
        account.deleted += group.deleted;
        results = account.results;
      }
      const nodeId = JSON.stringify([node.nodeId, appInfo.id, group.account]);
      let result = leaves.get(nodeId);
      if (!result) {
        result = {
          node: {
            ...node,
            nodeId,
            childCount: 0,
            bcpScope: node.bcpGroups ? { applications: [], account: group.account } : undefined,
            bcpPath: [evidenceLabel, app.label],
          },
          deleted: 0,
        };
        leaves.set(nodeId, result);
        results.push(result);
      }
      result.node.bcpScope?.applications.push(group.application);
      result.node.childCount += group.total;
      result.deleted += group.deleted;
      app.total += group.total;
      app.deleted += group.deleted;
    }
  }
  for (const app of apps.values()) {
    app.accounts.sort((left, right) => left.label.localeCompare(right.label));
    for (const account of app.accounts) {
      for (const result of account.results) result.node.bcpPath = [evidenceLabel, app.label, account.label];
    }
  }
  const ordered = [...apps.values()].sort((left, right) => {
    const deviceOrder = Number(right.icon === 'baseinfo.png') - Number(left.icon === 'baseinfo.png');
    return deviceOrder || Number(Boolean(right.icon)) - Number(Boolean(left.icon)) || left.label.localeCompare(right.label);
  });
  return { run, evidenceLabel, apps: ordered };
}

export function buildBcpDirectories(tree: AnalysisResultNode[], imageId: ImageId, tr: Translator = sourceText): BcpDirectory[] {
  const evidence = tree.find(node => node.kind === 'evidence' && node.imageId === imageId);
  if (!evidence) return [];
  return evidence.children.flatMap(category => category.children)
    .filter(run => run.kind === 'function' && run.pluginId === BCP_PLUGIN_ID && run.children.length > 0)
    .map(run => buildDirectory(run, evidence.label, tr));
}

export function bcpDirectoryResults(directory: BcpDirectory): AnalysisResultNode[] {
  return directory.apps.flatMap(app => [...app.results, ...app.accounts.flatMap(account => account.results)])
    .map(item => item.node);
}

export function searchBcpDirectory(
  directory: BcpDirectory, query: string, label: (node: AnalysisResultNode) => string,
): BcpDirectory | null {
  const text = query.trim().toLocaleLowerCase();
  if (!text) return directory;
  const matches = (value: string) => value.toLocaleLowerCase().includes(text);
  if (matches(directory.evidenceLabel) || matches(label(directory.run))) return directory;
  function filterResults(results: BcpResultItem[]) {
    return results.filter(result => matches(label(result.node)));
  }
  const apps = directory.apps.flatMap(app => {
    if (matches(app.label) || app.codes.some(matches)) return [app];
    const results = filterResults(app.results);
    const accounts = app.accounts.flatMap(account => {
      if (matches(account.label) || matches(account.account)) return [account];
      const results = filterResults(account.results);
      return results.length ? [{ ...account, results }] : [];
    });
    return results.length || accounts.length ? [{ ...app, results, accounts }] : [];
  });
  return apps.length ? { ...directory, apps } : null;
}
