// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest';
import { composeSafeHtml, escapeHtml, sanitizeHtml } from './safeHtml';
import { highlightHtml } from './highlight';

function parse(html: string): HTMLElement {
  const doc = new DOMParser().parseFromString(`<div id="root">${html}</div>`, 'text/html');
  const root = doc.getElementById('root');
  if (!root) throw new Error('parse failed');
  return root;
}

describe('escapeHtml', () => {
  it('escapes all five HTML metacharacters', () => {
    expect(escapeHtml(`<img src=x onerror="alert('1')">&`))
      .toBe('&lt;img src=x onerror=&quot;alert(&#39;1&#39;)&quot;&gt;&amp;');
  });

  it('composes already-safe fragments without re-escaping', () => {
    expect(composeSafeHtml(escapeHtml('a<b'), escapeHtml('&'))).toBe('a&lt;b&amp;');
  });
});

describe('sanitizeHtml', () => {
  it('replaces script-capable elements with their text', () => {
    // No `src` on the iframe: happy-dom would otherwise try to navigate it.
    const root = parse(sanitizeHtml('<b>ok</b><script>alert(1)</script><iframe></iframe><svg onload="x()">svg</svg>'));
    expect(root.querySelector('script, iframe, svg')).toBeNull();
    expect(root.querySelector('b')?.textContent).toBe('ok');
    expect(root.textContent).toBe('okalert(1)svg');
  });

  it('strips event handlers, inline styles and unknown attributes', () => {
    const span = parse(sanitizeHtml('<span class="k" onclick="x()" style="color:red" data-x="1">t</span>')).querySelector('span');
    expect(span?.getAttributeNames()).toEqual(['class']);
  });

  it('keeps only https / mailto links and forces safe link behaviour', () => {
    const links = [...parse(sanitizeHtml(
      '<a href="javascript:alert(1)">a</a>'
      + '<a href=" //evil.example/x">b</a>'
      + '<a href="https://example.com" target="_self">c</a>'
      + '<a href="mailto:a@example.com">d</a>',
    )).querySelectorAll('a')];
    expect(links.map(link => [link.textContent, link.getAttribute('href'), link.getAttribute('rel'), link.getAttribute('target')])).toEqual([
      ['a', null, 'noopener noreferrer', '_blank'],
      ['b', null, 'noopener noreferrer', '_blank'],
      ['c', 'https://example.com', 'noopener noreferrer', '_blank'],
      ['d', 'mailto:a@example.com', 'noopener noreferrer', '_blank'],
    ]);
  });

  it('scrubs nested payloads inside allowed elements', () => {
    const root = parse(sanitizeHtml('<p><em onmouseover="x()"><img src=x onerror="alert(1)">hi</em></p>'));
    expect(root.querySelector('img')).toBeNull();
    expect(root.querySelector('em')?.getAttributeNames()).toEqual([]);
    expect(root.textContent).toBe('hi');
  });
});

describe('highlightHtml', () => {
  it('escapes the text around highlighted matches', () => {
    expect(highlightHtml('<b>x</b> and X', 'x')).toEqual({
      html: '&lt;b&gt;<mark>x</mark>&lt;/b&gt; and <mark>X</mark>',
      count: 2,
    });
  });

  it('matches queries with HTML and regex metacharacters literally', () => {
    expect(highlightHtml('a<b a.b a<b', 'a<b')).toEqual({
      html: '<mark>a&lt;b</mark> a.b <mark>a&lt;b</mark>',
      count: 2,
    });
  });

  it('only finds text present before HTML escaping', () => {
    expect(highlightHtml('& amp <', 'amp')).toEqual({
      html: '&amp; <mark>amp</mark> &lt;',
      count: 1,
    });
  });
});
