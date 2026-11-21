// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { createApp, h, type App } from 'vue';
import FileNodeIcon from './FileNodeIcon.vue';

const apps: App[] = [];

afterEach(() => {
  for (const app of apps.splice(0)) app.unmount();
  document.body.innerHTML = '';
});

function render(props: { id: string; kind: string; name: string; expanded?: boolean }): SVGElement {
  const host = document.createElement('div');
  document.body.append(host);
  const app = createApp({ render: () => h(FileNodeIcon, { ...props, size: 15 }) });
  apps.push(app);
  app.mount(host);
  const svg = host.querySelector('svg');
  if (!svg) throw new Error('FileNodeIcon rendered no svg');
  return svg;
}

const iconOf = (props: { id: string; kind: string; name: string; expanded?: boolean }) => render(props).getAttribute('data-icon');

describe('FileNodeIcon', () => {
  it('draws the special evidence nodes before looking at the name', () => {
    const cases = [
      { id: 'part:2', kind: 'dir', name: 'Basic data partition' },
      { id: 'lvm:0:1', kind: 'dir', name: 'root' },
      { id: 'unalloc:part:2', kind: 'unallocated', name: 'Unallocated' },
      { id: 'diskgap:0:2048', kind: 'file', name: 'gap.bin' },
      { id: 'host-docker:', kind: 'dir', name: 'Docker' },
      { id: 'host-docker:abc:', kind: 'dir', name: 'web' },
      { id: 'refs-snap:5:7', kind: 'file', name: 'report.docx (snapshot)' },
      { id: 'ntfs-ads:5:7', kind: 'file', name: 'report.docx:Zone.Identifier' },
      { id: '5', kind: 'symlink', name: 'latest.exe' },
    ];
    expect(cases.map(iconOf)).toEqual([
      'hard_drive', 'hard_drive', 'unallocated', 'unallocated', 'docker', 'docker', 'history_rerun', 'layers', 'link',
    ]);
  });

  it('opens expanded folders and keeps the category color class on files', () => {
    expect([
      iconOf({ id: '10', kind: 'dir', name: 'Windows' }),
      iconOf({ id: '11', kind: 'dir', name: 'Windows', expanded: true }),
    ]).toEqual(['folder', 'folder_open']);
    const file = render({ id: '12', kind: 'file', name: 'SETUP.EXE' });
    expect({ icon: file.getAttribute('data-icon'), classes: [...file.classList] })
      .toEqual({ icon: 'binary', classes: ['file-node-icon', 'kind-file', 'cat-executable'] });
  });

  it('inlines the icon at the requested size, painted with currentColor', () => {
    const svg = render({ id: '13', kind: 'file', name: 'main.rs' });
    expect({
      width: svg.getAttribute('width'),
      height: svg.getAttribute('height'),
      viewBox: svg.getAttribute('viewBox'),
      fill: svg.getAttribute('fill'),
      hidden: svg.getAttribute('aria-hidden'),
      painted: svg.querySelectorAll('[fill="currentColor"], [stroke="currentColor"]').length > 0,
      placeholderColors: /black|#[0-9a-f]{3,6}\b/i.test(svg.innerHTML),
    }).toEqual({ width: '15', height: '15', viewBox: '0 0 16 16', fill: 'none', hidden: 'true', painted: true, placeholderColors: false });
  });
});
