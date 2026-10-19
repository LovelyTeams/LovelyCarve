import { describe, expect, it } from 'vitest';
// The verifier runs in Node/Bun; these fixtures exercise the same AST rules.
import { auditModule, importTarget, runtimeCycles } from '../scripts/lib/frontend-structure.mjs';

describe('frontend architecture verifier', () => {
  it('detects multi-module and self cycles without flagging a shared leaf', () => {
    const graph = new Map([['a', ['b', 'leaf']], ['b', ['a']], ['c', ['leaf']], ['leaf', []], ['self', ['self']]]);
    expect(runtimeCycles(graph)).toEqual([['a', 'b'], ['self']]);
  });
  it('resolves aliases and relative paths identically', () => {
    expect(importTarget('src/features/files/components/Page.vue', '@/features/preview/api')).toBe('src/features/preview/api');
    expect(importTarget('src/features/files/components/Page.vue', '../../preview/api')).toBe('src/features/preview/api');
  });
  it('rejects reverse and private dynamic dependencies in both Vue script blocks', () => {
    const result = auditModule('src/features/files/components/Page.vue', `<script lang="ts">import x from '@/app/workspace/panels';</script><script setup lang="ts">const panel = () => import('../../search/composables/useSearchHits');</script>`);
    expect(result.violations.map(item => item.message)).toEqual([
      'A feature must not depend on app/.',
      'Cross-feature imports must use an explicit API, component entry, or pure contract.',
    ]);
  });
  it('distinguishes type-only imports and rejects runtime IPC in pure models', () => {
    const result = auditModule('src/features/files/model/example.ts', `import type { FsNode } from '@/lib/ipc/fs/types'; import { fsChildren } from '@/lib/ipc/fs/query';`);
    expect(result.imports.map(item => item.typeOnly)).toEqual([true, false]);
    expect(result.violations).toHaveLength(1);
  });
  it('ignores host words in comments and property names, but catches host side effects', () => {
    const source = `// localStorage is not used\nconst groups = { document: '文档' }; const name = localStorage.getItem('key');`;
    const result = auditModule('src/features/carving/model/example.ts', source);
    expect(result.violations.map(item => item.message)).toEqual(['Pure model uses host global localStorage.']);
  });
  it('allows declared component and API edges and catches self-barrel loops', () => {
    expect(auditModule('src/features/files/components/Page.vue', `<script setup lang="ts">import Preview from '../../preview/components/DiskFileDetail.vue'; import { useCaseSession } from '../../cases/api';</script>`).violations).toEqual([]);
    expect(auditModule('src/features/tasks/composables/example.ts', `export { useTaskCenter } from '../api';`).violations[0].message).toContain('own API barrel');
  });
});
