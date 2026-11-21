import { describe, expect, it } from 'vitest';
import { inlineSvgIcon } from './svgIcon';

describe('inlineSvgIcon', () => {
  it('paints every stroke and fill with currentColor', () => {
    const icon = inlineSvgIcon(
      '<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg">\n'
      + '<path d="M3 5H11" stroke="black" stroke-width="1.2" stroke-linecap="round"/>\n'
      + '<path opacity="0.8" fill-rule="evenodd" d="M1 1h2v2z" fill="#DCE0E5"/>\n'
      + '<rect fill="none" stroke="#C6CAD0" width="2" height="2"/>\n'
      + '</svg>\n',
      'p-',
    );
    expect(icon).toEqual({
      viewBox: '0 0 16 16',
      fill: 'none',
      body: '<path d="M3 5H11" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>\n'
        + '<path opacity="0.8" fill-rule="evenodd" d="M1 1h2v2z" fill="currentColor"/>\n'
        + '<rect fill="none" stroke="currentColor" width="2" height="2"/>',
    });
  });

  it('recolors inline styles and keeps their other properties', () => {
    const icon = inlineSvgIcon('<svg width="16" height="16" fill="none"><g style="fill:#000;fill-opacity:1" fill="#180c25"><path style="fill: #000; stroke-width:.25"/></g></svg>', 'p-');
    expect(icon.body).toBe('<g style="fill:currentColor;fill-opacity:1" fill="currentColor"><path style="fill:currentColor; stroke-width:.25"/></g>');
  });

  it('keeps mask and clip-path geometry and prefixes their ids', () => {
    const icon = inlineSvgIcon(
      '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" fill="none"><g clip-path="url(#a)">'
      + '<mask id="b" style="mask-type:luminance"><path fill="#fff" d="M0 0h8v8H0z"/></mask>'
      + '<g fill="#c6cad0" mask="url(#b)"><path d="M1 1h4"/></g></g>'
      + '<defs><clipPath id="a"><path fill="#fff" d="M0 0h16v16H0z"/></clipPath></defs></svg>',
      'icon-',
    );
    expect(icon).toEqual({
      viewBox: '0 0 16 16',
      fill: 'none',
      body: '<g clip-path="url(#icon-a)">'
        + '<mask id="icon-b" style="mask-type:luminance"><path fill="#fff" d="M0 0h8v8H0z"/></mask>'
        + '<g fill="currentColor" mask="url(#icon-b)"><path d="M1 1h4"/></g></g>'
        + '<defs><clipPath id="icon-a"><path fill="#fff" d="M0 0h16v16H0z"/></clipPath></defs>',
    });
  });

  it('derives the view box and the root paint when the source omits them', () => {
    const icon = inlineSvgIcon('<svg xmlns="http://www.w3.org/2000/svg" width="24" height="20" stroke="#000" stroke-width="2"><path d="M1 1h4"/></svg>', 'p-');
    expect(icon).toEqual({
      viewBox: '0 0 24 20',
      fill: 'currentColor',
      body: '<g stroke="currentColor" stroke-width="2"><path d="M1 1h4"/></g>',
    });
  });

  it('rejects sources that are not a single svg element', () => {
    expect(() => inlineSvgIcon('<div></div>', 'p-')).toThrow();
  });
});
