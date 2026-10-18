// Chunk placement of md arrays, for the stripe-layout diagram. Mirrors the
// backend readers: RAID4/5/6 follow md's per-row parity rotation (every layout
// code md defines), RAID10 its near / far / offset copy placement.

export type CellKind = 'data' | 'copy' | 'p' | 'q' | 'empty';

export interface StripeCell {
  kind: CellKind;
  /** Logical chunk shown in a data or copy cell. */
  dataIndex?: number;
}

/** Levels the diagram can draw. */
export function hasStripeLayout(level: number): boolean {
  return [0, 1, 4, 5, 6, 10].includes(level);
}

function mod(a: number, n: number): number {
  return ((a % n) + n) % n;
}

interface Placement {
  p: number;
  q: number;
  column: number;
}

/** Column of data position `position` in stripe row `stripe`, plus the row's
 *  P and Q columns (`q` is -1 without Q). `null` for unknown layouts. */
function place(level: number, layout: number, n: number, stripe: number, position: number): Placement | null {
  const skip = (parity: number) => (position >= parity ? position + 1 : position);
  if (level === 4) return { p: n - 1, q: -1, column: position };
  if (level === 5) {
    const dataDisks = n - 1;
    switch (layout) {
      case 0: { const p = dataDisks - mod(stripe, n); return { p, q: -1, column: skip(p) }; }
      case 1: { const p = mod(stripe, n); return { p, q: -1, column: skip(p) }; }
      case 2: { const p = dataDisks - mod(stripe, n); return { p, q: -1, column: mod(p + 1 + position, n) }; }
      case 3: { const p = mod(stripe, n); return { p, q: -1, column: mod(p + 1 + position, n) }; }
      case 4: return { p: 0, q: -1, column: position + 1 };
      case 5: return { p: dataDisks, q: -1, column: position };
      default: return null;
    }
  }
  if (level === 6) {
    const dataDisks = n - 2;
    // Q follows P, except when P is the last column: Q wraps to 0 and data starts at 1.
    const asymmetric = (p: number): Placement => (p === n - 1
      ? { p, q: 0, column: position + 1 }
      : { p, q: p + 1, column: position >= p ? position + 2 : position });
    switch (layout) {
      case 0: return asymmetric(n - 1 - mod(stripe, n));
      case 1: return asymmetric(mod(stripe, n));
      case 2: { const p = n - 1 - mod(stripe, n); return { p, q: mod(p + 1, n), column: mod(p + 2 + position, n) }; }
      case 3: { const p = mod(stripe, n); return { p, q: mod(p + 1, n), column: mod(p + 2 + position, n) }; }
      case 4: return { p: 0, q: 1, column: position + 2 };
      case 5: return { p: dataDisks, q: dataDisks + 1, column: position };
      case 8: return asymmetric(mod(stripe, n));
      case 9: return asymmetric(n - 1 - mod(stripe + 1, n));
      case 10: { const p = n - 1 - mod(stripe, n); return { p, q: mod(p + n - 1, n), column: mod(p + 1 + position, n) }; }
      case 16: { const p = dataDisks - mod(stripe, n - 1); return { p, q: n - 1, column: skip(p) }; }
      case 17: { const p = mod(stripe, n - 1); return { p, q: n - 1, column: skip(p) }; }
      case 18: { const p = dataDisks - mod(stripe, n - 1); return { p, q: n - 1, column: mod(p + 1 + position, n - 1) }; }
      case 19: { const p = mod(stripe, n - 1); return { p, q: n - 1, column: mod(p + 1 + position, n - 1) }; }
      case 20: return { p: 0, q: n - 1, column: position + 1 };
      default: return null;
    }
  }
  return null;
}

/** RAID10 layout fields: `near | far << 8 | offset << 16 | far sets << 17`. */
export interface Raid10Copies {
  near: number;
  far: number;
  offset: boolean;
}

export function decodeRaid10Layout(layout: number): Raid10Copies {
  return { near: layout & 0xff, far: (layout >> 8) & 0xff, offset: (layout & (1 << 16)) !== 0 };
}

export function encodeRaid10Layout(copies: Raid10Copies): number {
  return (copies.near & 0xff) | ((copies.far & 0xff) << 8) | (copies.offset ? 1 << 16 : 0);
}

/** `(device, row)` of every copy of logical chunk `chunk` (md's
 *  `__raid10_find_phys`). Far copies sit `strideRows` rows further down: md
 *  puts them a device-fraction away, the diagram a few rows. */
function raid10Copies(layout: number, n: number, chunk: number, strideRows: number): Array<[number, number]> {
  const { near, far, offset } = decodeRaid10Layout(layout);
  const sets = layout >> 17;
  const farSetSize = sets === 1 ? Math.floor(n / far) : sets === 2 ? far * near : n;
  if (near < 1 || far < 1 || farSetSize < 1 || farSetSize > n) return [];
  const lastFarSetStart = (Math.floor(n / farSetSize) - 1) * farSetSize;
  const lastFarSetSize = farSetSize + (n % farSetSize);
  const scaled = chunk * near;
  let stripe = Math.floor(scaled / n);
  let device = scaled % n;
  if (offset) stripe *= far;
  let row = stripe;
  const out: Array<[number, number]> = [];
  for (let copy = 0; copy < near; copy++) {
    let copyDevice = device;
    let copyRow = row;
    out.push([copyDevice, copyRow]);
    for (let f = 1; f < far; f++) {
      const set = Math.floor(copyDevice / farSetSize);
      copyDevice += near;
      if (n % farSetSize !== 0 && copyDevice > lastFarSetStart) {
        copyDevice = mod(copyDevice - lastFarSetStart, lastFarSetSize) + lastFarSetStart;
      } else {
        copyDevice = mod(copyDevice, farSetSize) + farSetSize * set;
      }
      copyRow += offset ? 1 : strideRows;
      out.push([copyDevice, copyRow]);
    }
    device += 1;
    if (device >= n) {
      device = 0;
      row += 1;
    }
  }
  return out;
}

/** Rows to draw: one full parity rotation (capped), or enough RAID10 rows to
 *  show every far copy. */
export function stripeRows(level: number, layout: number, n: number): number {
  if (level === 0 || level === 1) return 4;
  if (level === 10) {
    const { far } = decodeRaid10Layout(layout);
    return Math.max(far, 1) * Math.max(2, Math.min(4, n));
  }
  return Math.min(Math.max(n, 4), 8);
}

/** Cells of the first `rows` chunk rows of an `n`-member array. */
export function stripeGrid(level: number, layout: number, n: number, rows: number): StripeCell[][] {
  const grid: StripeCell[][] = Array.from({ length: rows }, () =>
    Array.from({ length: n }, () => ({ kind: 'empty' as CellKind })));
  if (n < 1) return grid;
  if (level === 0) {
    for (let r = 0; r < rows; r++) for (let c = 0; c < n; c++) grid[r][c] = { kind: 'data', dataIndex: r * n + c };
    return grid;
  }
  if (level === 1) {
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < n; c++) grid[r][c] = { kind: c === 0 ? 'data' : 'copy', dataIndex: r };
    }
    return grid;
  }
  if (level === 10) {
    const { near, far, offset } = decodeRaid10Layout(layout);
    const copies = near * far;
    if (copies < 1) return grid;
    const strideRows = offset ? 1 : Math.max(1, Math.floor(rows / Math.max(far, 1)));
    const chunks = Math.floor((rows * n) / copies);
    for (let chunk = 0; chunk < chunks; chunk++) {
      raid10Copies(layout, n, chunk, strideRows).forEach(([device, row], index) => {
        if (row < rows && device < n && grid[row][device].kind === 'empty') {
          grid[row][device] = { kind: index === 0 ? 'data' : 'copy', dataIndex: chunk };
        }
      });
    }
    return grid;
  }
  const parityDisks = level === 6 ? 2 : 1;
  const dataDisks = n - parityDisks;
  if (dataDisks < 1) return grid;
  for (let r = 0; r < rows; r++) {
    for (let position = 0; position < dataDisks; position++) {
      const placed = place(level, layout, n, r, position);
      if (!placed || placed.column >= n || placed.p >= n) return grid;
      grid[r][placed.p] = { kind: 'p' };
      if (placed.q >= 0 && placed.q < n) grid[r][placed.q] = { kind: 'q' };
      grid[r][placed.column] = { kind: 'data', dataIndex: r * dataDisks + position };
    }
  }
  return grid;
}
