// 结果树、列与枚举值的名称:插件声明的标签按界面语言取包内译文(插件已卸载时
// 保留存下的标签)。查表在调用方的 computed / 渲染里进行,语言与插件目录变化
// 时随之重算。
import { tr } from '../../../composables/useLocale';
import type { AnalysisResultNode, RecordColumn } from '../../../lib/ipc/analysisResults';
import { categoryLabel, pluginTextKey } from '../../../lib/models/pluginText';
import { usePluginCatalog } from '../../tasks/api';
import type { AnalysisNodeLabels } from '../model/analysisResultView';

export function useAnalysisLabels() {
  const catalog = usePluginCatalog();

  function nodeLabel(node: AnalysisResultNode): string {
    switch (node.kind) {
      case 'function': return catalog.text(node.pluginId, pluginTextKey.analyzer(node.analyzer), node.label);
      case 'result': return catalog.text(node.pluginId, pluginTextKey.record(node.artifactType), node.label);
      case 'class': return categoryLabel(node.typeName, node.pluginId, catalog.text, tr);
      case 'evidence': return node.label;
    }
  }

  const nodeLabels: AnalysisNodeLabels = {
    node: nodeLabel,
    category: node => categoryLabel(node.typeName, node.pluginId, catalog.text, tr),
  };

  /** A column of a record type of `node`'s plugin. */
  function columnLabel(node: AnalysisResultNode | null, column: RecordColumn): string {
    if (!node) return column.label;
    return catalog.text(node.pluginId, pluginTextKey.field(node.artifactType, column.id), column.label);
  }

  /** The name of an enum value of `column`. */
  function choiceLabel(node: AnalysisResultNode | null, column: RecordColumn, choice: string): string {
    const declared = column.choices.find(item => item.id === choice)?.label ?? choice;
    if (!node) return declared;
    return catalog.text(node.pluginId, pluginTextKey.fieldChoice(node.artifactType, column.id, choice), declared);
  }

  return { nodeLabels, nodeLabel, columnLabel, choiceLabel };
}
