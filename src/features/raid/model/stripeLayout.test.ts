import { describe, expect, it } from 'vitest';
import { decodeRaid10Layout, encodeRaid10Layout, stripeGrid, type StripeCell } from './stripeLayout';

function chart(level: number, layout: number, n: number, rows: number): string[] {
  const label = (cell: StripeCell) => {
    if (cell.kind === 'p') return 'P';
    if (cell.kind === 'q') return 'Q';
    if (cell.kind === 'data') return `D${cell.dataIndex}`;
    if (cell.kind === 'copy') return `d${cell.dataIndex}`;
    return '.';
  };
  return stripeGrid(level, layout, n, rows).map(row => row.map(label).join(' '));
}

describe('md stripe layouts', () => {
  it('rotates RAID5 parity like md', () => {
    expect(chart(5, 2, 3, 3)).toEqual(['D0 D1 P', 'D3 P D2', 'P D4 D5']);
    expect(chart(5, 0, 3, 3)).toEqual(['D0 D1 P', 'D2 P D3', 'P D4 D5']);
    expect(chart(5, 3, 3, 3)).toEqual(['P D0 D1', 'D3 P D2', 'D4 D5 P']);
    expect(chart(5, 1, 3, 3)).toEqual(['P D0 D1', 'D2 P D3', 'D4 D5 P']);
    expect(chart(5, 4, 3, 2)).toEqual(['P D0 D1', 'P D2 D3']);
    expect(chart(4, 0, 3, 2)).toEqual(['D0 D1 P', 'D2 D3 P']);
  });

  it('places RAID6 P and Q like md', () => {
    expect(chart(6, 2, 4, 4)).toEqual(['Q D0 D1 P', 'D2 D3 P Q', 'D5 P Q D4', 'P Q D6 D7']);
    expect(chart(6, 0, 5, 3)).toEqual(['Q D0 D1 D2 P', 'D3 D4 D5 P Q', 'D6 D7 P Q D8']);
    expect(chart(6, 5, 4, 2)).toEqual(['D0 D1 P Q', 'D2 D3 P Q']);
    expect(chart(6, 18, 4, 3)).toEqual(['D0 D1 P Q', 'D3 P D2 Q', 'P D4 D5 Q']);
    expect(chart(6, 10, 4, 2)).toEqual(['D0 D1 Q P', 'D3 Q P D2']);
  });

  it('places RAID10 copies like md', () => {
    expect(chart(10, 0x102, 4, 2)).toEqual(['D0 d0 D1 d1', 'D2 d2 D3 d3']);
    expect(chart(10, 0x201, 2, 4)).toEqual(['D0 D1', 'D2 D3', 'd1 d0', 'd3 d2']);
    expect(chart(10, 0x10201, 2, 4)).toEqual(['D0 D1', 'd1 d0', 'D2 D3', 'd3 d2']);
  });

  it('mirrors and stripes plain levels', () => {
    expect(chart(1, 0, 2, 2)).toEqual(['D0 d0', 'D1 d1']);
    expect(chart(0, 0, 3, 2)).toEqual(['D0 D1 D2', 'D3 D4 D5']);
  });

  it('round-trips the RAID10 layout field', () => {
    for (const layout of [0x102, 0x201, 0x10201, 0x103]) {
      expect(encodeRaid10Layout(decodeRaid10Layout(layout))).toBe(layout);
    }
  });
});
