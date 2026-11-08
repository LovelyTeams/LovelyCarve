//! Placement of data, P and Q chunks in md RAID4 / RAID5 / RAID6 stripe rows.
//!
//! The arithmetic follows the kernel's `raid5_compute_sector` for every layout
//! code md defines: logical chunk `c` lives in stripe row `c / data_disks` at
//! data position `c % data_disks`, and the layout decides which physical column
//! holds P, Q and each data position of that row. RAID6 additionally fixes the
//! order of the data columns in the Q syndrome.

/// md RAID5 `layout` codes (also used for RAID4 conversions).
///
/// 0/1: the parity column rotates and data fills the remaining columns in
/// column order ("asymmetric"). 2/3: data starts right after parity and wraps
/// ("symmetric"). "Left" rotates parity from the last column backwards,
/// "right" from the first column forwards. 4/5 keep parity on the first / last
/// column (RAID4 shapes). `2` (left-symmetric) is md's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raid5Layout {
    LeftAsymmetric,
    RightAsymmetric,
    LeftSymmetric,
    RightSymmetric,
    Parity0,
    ParityN,
}

impl Raid5Layout {
    pub fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::LeftAsymmetric),
            1 => Some(Self::RightAsymmetric),
            2 => Some(Self::LeftSymmetric),
            3 => Some(Self::RightSymmetric),
            4 => Some(Self::Parity0),
            5 => Some(Self::ParityN),
            _ => None,
        }
    }

    pub fn raw(&self) -> u32 {
        match self {
            Self::LeftAsymmetric => 0,
            Self::RightAsymmetric => 1,
            Self::LeftSymmetric => 2,
            Self::RightSymmetric => 3,
            Self::Parity0 => 4,
            Self::ParityN => 5,
        }
    }

    /// Lowercase ASCII label used in diagnostics.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LeftAsymmetric => "left-asymmetric",
            Self::RightAsymmetric => "right-asymmetric",
            Self::LeftSymmetric => "left-symmetric",
            Self::RightSymmetric => "right-symmetric",
            Self::Parity0 => "parity-first",
            Self::ParityN => "parity-last",
        }
    }
}

/// md RAID6 `layout` codes.
///
/// 0–3 rotate P with Q in the next column; 4/5 keep P and Q on the first /
/// last two columns; 8–10 are the DDF rotations, which also compute Q over the
/// data columns in column order; 16–20 are the RAID5 layouts with Q fixed on
/// the last column (what a RAID5 → RAID6 conversion produces).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raid6Layout {
    LeftAsymmetric,
    RightAsymmetric,
    LeftSymmetric,
    RightSymmetric,
    Parity0,
    ParityN,
    RotatingZeroRestart,
    RotatingNRestart,
    RotatingNContinue,
    LeftAsymmetric6,
    RightAsymmetric6,
    LeftSymmetric6,
    RightSymmetric6,
    Parity06,
}

impl Raid6Layout {
    pub fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::LeftAsymmetric),
            1 => Some(Self::RightAsymmetric),
            2 => Some(Self::LeftSymmetric),
            3 => Some(Self::RightSymmetric),
            4 => Some(Self::Parity0),
            5 => Some(Self::ParityN),
            8 => Some(Self::RotatingZeroRestart),
            9 => Some(Self::RotatingNRestart),
            10 => Some(Self::RotatingNContinue),
            16 => Some(Self::LeftAsymmetric6),
            17 => Some(Self::RightAsymmetric6),
            18 => Some(Self::LeftSymmetric6),
            19 => Some(Self::RightSymmetric6),
            20 => Some(Self::Parity06),
            _ => None,
        }
    }

    pub fn raw(&self) -> u32 {
        match self {
            Self::LeftAsymmetric => 0,
            Self::RightAsymmetric => 1,
            Self::LeftSymmetric => 2,
            Self::RightSymmetric => 3,
            Self::Parity0 => 4,
            Self::ParityN => 5,
            Self::RotatingZeroRestart => 8,
            Self::RotatingNRestart => 9,
            Self::RotatingNContinue => 10,
            Self::LeftAsymmetric6 => 16,
            Self::RightAsymmetric6 => 17,
            Self::LeftSymmetric6 => 18,
            Self::RightSymmetric6 => 19,
            Self::Parity06 => 20,
        }
    }

    /// Lowercase ASCII label used in diagnostics.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LeftAsymmetric => "left-asymmetric",
            Self::RightAsymmetric => "right-asymmetric",
            Self::LeftSymmetric => "left-symmetric",
            Self::RightSymmetric => "right-symmetric",
            Self::Parity0 => "parity-first",
            Self::ParityN => "parity-last",
            Self::RotatingZeroRestart => "ddf-zero-restart",
            Self::RotatingNRestart => "ddf-n-restart",
            Self::RotatingNContinue => "ddf-n-continue",
            Self::LeftAsymmetric6 => "left-asymmetric-6",
            Self::RightAsymmetric6 => "right-asymmetric-6",
            Self::LeftSymmetric6 => "left-symmetric-6",
            Self::RightSymmetric6 => "right-symmetric-6",
            Self::Parity06 => "parity-first-6",
        }
    }
}

/// Which parity family a stripe row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParityKind {
    Raid4,
    Raid5(Raid5Layout),
    Raid6(Raid6Layout),
}

impl ParityKind {
    pub(crate) fn parity_disks(&self) -> u64 {
        match self {
            Self::Raid4 | Self::Raid5(_) => 1,
            Self::Raid6(_) => 2,
        }
    }
}

/// Where one data position of a stripe row lives, plus the row's parity
/// columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Placement {
    pub p: usize,
    pub q: Option<usize>,
    pub column: usize,
    /// DDF layouts compute Q over the data columns in column order.
    pub ddf: bool,
}

/// Column of data position `position` in stripe row `stripe` of an
/// `n`-column array. The caller guarantees `n` exceeds the parity count and
/// `position < n - parity_disks`.
pub(crate) fn place(kind: ParityKind, n: u64, stripe: u64, position: u64) -> Placement {
    let data_disks = n - kind.parity_disks();
    let (p, q, column, ddf) = match kind {
        ParityKind::Raid4 => (data_disks, None, position, false),
        ParityKind::Raid5(layout) => {
            let (p, column) = match layout {
                Raid5Layout::LeftAsymmetric => {
                    let p = data_disks - stripe % n;
                    (p, skip_one(position, p))
                }
                Raid5Layout::RightAsymmetric => {
                    let p = stripe % n;
                    (p, skip_one(position, p))
                }
                Raid5Layout::LeftSymmetric => {
                    let p = data_disks - stripe % n;
                    (p, (p + 1 + position) % n)
                }
                Raid5Layout::RightSymmetric => {
                    let p = stripe % n;
                    (p, (p + 1 + position) % n)
                }
                Raid5Layout::Parity0 => (0, position + 1),
                Raid5Layout::ParityN => (data_disks, position),
            };
            (p, None, column, false)
        }
        ParityKind::Raid6(layout) => {
            let (p, q, column, ddf) = raid6_place(layout, n, data_disks, stripe, position);
            (p, Some(q), column, ddf)
        }
    };
    Placement {
        p: p as usize,
        q: q.map(|q| q as usize),
        column: column as usize,
        ddf,
    }
}

/// Asymmetric data placement: positions at or past the parity column shift
/// one column right.
fn skip_one(position: u64, parity: u64) -> u64 {
    if position >= parity {
        position + 1
    } else {
        position
    }
}

/// `(p, q, column, ddf)` for a RAID6 row.
fn raid6_place(
    layout: Raid6Layout,
    n: u64,
    data_disks: u64,
    stripe: u64,
    position: u64,
) -> (u64, u64, u64, bool) {
    // Left/right asymmetric share this: Q follows P, except when P is the last
    // column, where Q wraps to column 0 and data starts at column 1.
    let asymmetric = |p: u64| {
        if p == n - 1 {
            (p, 0, position + 1)
        } else {
            let column = if position >= p {
                position + 2
            } else {
                position
            };
            (p, p + 1, column)
        }
    };
    match layout {
        Raid6Layout::LeftAsymmetric => {
            let (p, q, column) = asymmetric(n - 1 - stripe % n);
            (p, q, column, false)
        }
        Raid6Layout::RightAsymmetric => {
            let (p, q, column) = asymmetric(stripe % n);
            (p, q, column, false)
        }
        Raid6Layout::LeftSymmetric => {
            let p = n - 1 - stripe % n;
            (p, (p + 1) % n, (p + 2 + position) % n, false)
        }
        Raid6Layout::RightSymmetric => {
            let p = stripe % n;
            (p, (p + 1) % n, (p + 2 + position) % n, false)
        }
        Raid6Layout::Parity0 => (0, 1, position + 2, false),
        Raid6Layout::ParityN => (data_disks, data_disks + 1, position, false),
        Raid6Layout::RotatingZeroRestart => {
            let (p, q, column) = asymmetric(stripe % n);
            (p, q, column, true)
        }
        Raid6Layout::RotatingNRestart => {
            let (p, q, column) = asymmetric(n - 1 - (stripe + 1) % n);
            (p, q, column, true)
        }
        Raid6Layout::RotatingNContinue => {
            let p = n - 1 - stripe % n;
            (p, (p + n - 1) % n, (p + 1 + position) % n, true)
        }
        Raid6Layout::LeftAsymmetric6 => {
            let p = data_disks - stripe % (n - 1);
            (p, n - 1, skip_one(position, p), false)
        }
        Raid6Layout::RightAsymmetric6 => {
            let p = stripe % (n - 1);
            (p, n - 1, skip_one(position, p), false)
        }
        Raid6Layout::LeftSymmetric6 => {
            let p = data_disks - stripe % (n - 1);
            (p, n - 1, (p + 1 + position) % (n - 1), false)
        }
        Raid6Layout::RightSymmetric6 => {
            let p = stripe % (n - 1);
            (p, n - 1, (p + 1 + position) % (n - 1), false)
        }
        Raid6Layout::Parity06 => (0, n - 1, position + 1, false),
    }
}

/// Every column of one stripe row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowPlacement {
    pub p: usize,
    pub q: Option<usize>,
    /// Physical column of each data position, in logical order.
    pub data: Vec<usize>,
    pub ddf: bool,
}

impl RowPlacement {
    pub(crate) fn new(kind: ParityKind, n: u64, stripe: u64) -> Self {
        let data_disks = n - kind.parity_disks();
        let first = place(kind, n, stripe, 0);
        let data = (0..data_disks)
            .map(|position| place(kind, n, stripe, position).column)
            .collect();
        Self {
            p: first.p,
            q: first.q,
            data,
            ddf: first.ddf,
        }
    }

    /// Exponent of a data column's coefficient in this row's Q syndrome.
    ///
    /// md walks the columns starting just after Q (column 0 when Q is the last
    /// column) and numbers the data columns in that order; DDF layouts number
    /// every column by its index.
    pub(crate) fn q_exponent(&self, column: usize, n: usize) -> usize {
        if self.ddf {
            return column;
        }
        let Some(q) = self.q else {
            return 0;
        };
        let mut current = if q + 1 >= n { 0 } else { q + 1 };
        let mut exponent = 0;
        for _ in 0..n {
            if current == column {
                break;
            }
            if current != self.p && current != q {
                exponent += 1;
            }
            current = (current + 1) % n;
        }
        exponent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render rows as the familiar layout charts: `D<n>` data, `P`, `Q`.
    fn chart(kind: ParityKind, n: u64, rows: u64) -> Vec<String> {
        let data_disks = n - kind.parity_disks();
        (0..rows)
            .map(|stripe| {
                let row = RowPlacement::new(kind, n, stripe);
                let mut cells = vec![String::new(); n as usize];
                cells[row.p] = "P".into();
                if let Some(q) = row.q {
                    cells[q] = "Q".into();
                }
                for (position, &column) in row.data.iter().enumerate() {
                    cells[column] = format!("D{}", stripe * data_disks + position as u64);
                }
                cells.join(" ")
            })
            .collect()
    }

    #[test]
    fn raid5_layouts_match_md_charts() {
        assert_eq!(
            chart(ParityKind::Raid5(Raid5Layout::LeftSymmetric), 3, 3),
            ["D0 D1 P", "D3 P D2", "P D4 D5"]
        );
        assert_eq!(
            chart(ParityKind::Raid5(Raid5Layout::LeftAsymmetric), 3, 3),
            ["D0 D1 P", "D2 P D3", "P D4 D5"]
        );
        assert_eq!(
            chart(ParityKind::Raid5(Raid5Layout::RightSymmetric), 3, 3),
            ["P D0 D1", "D3 P D2", "D4 D5 P"]
        );
        assert_eq!(
            chart(ParityKind::Raid5(Raid5Layout::RightAsymmetric), 3, 3),
            ["P D0 D1", "D2 P D3", "D4 D5 P"]
        );
        assert_eq!(
            chart(ParityKind::Raid5(Raid5Layout::Parity0), 3, 2),
            ["P D0 D1", "P D2 D3"]
        );
        assert_eq!(chart(ParityKind::Raid4, 3, 2), ["D0 D1 P", "D2 D3 P"]);
    }

    #[test]
    fn raid6_left_symmetric_matches_md_chart() {
        assert_eq!(
            chart(ParityKind::Raid6(Raid6Layout::LeftSymmetric), 4, 4),
            ["Q D0 D1 P", "D2 D3 P Q", "D5 P Q D4", "P Q D6 D7"]
        );
    }

    #[test]
    fn raid6_asymmetric_and_fixed_layouts_match_md_charts() {
        assert_eq!(
            chart(ParityKind::Raid6(Raid6Layout::LeftAsymmetric), 5, 5),
            [
                "Q D0 D1 D2 P",
                "D3 D4 D5 P Q",
                "D6 D7 P Q D8",
                "D9 P Q D10 D11",
                "P Q D12 D13 D14"
            ]
        );
        assert_eq!(
            chart(ParityKind::Raid6(Raid6Layout::ParityN), 4, 2),
            ["D0 D1 P Q", "D2 D3 P Q"]
        );
        assert_eq!(
            chart(ParityKind::Raid6(Raid6Layout::LeftSymmetric6), 4, 3),
            ["D0 D1 P Q", "D3 P D2 Q", "P D4 D5 Q"]
        );
        assert_eq!(
            chart(ParityKind::Raid6(Raid6Layout::RotatingNContinue), 4, 2),
            ["D0 D1 Q P", "D3 Q P D2"]
        );
    }

    #[test]
    fn q_exponents_number_data_columns_after_q() {
        // Left-symmetric row 0 of four columns is "Q D0 D1 P": the walk starts
        // after Q, so D0 and D1 carry g^0 and g^1.
        let row = RowPlacement::new(ParityKind::Raid6(Raid6Layout::LeftSymmetric), 4, 0);
        assert_eq!(row.q_exponent(1, 4), 0);
        assert_eq!(row.q_exponent(2, 4), 1);
        // Row 1 is "D2 D3 P Q": Q is the last column, so the walk starts at 0.
        let row = RowPlacement::new(ParityKind::Raid6(Raid6Layout::LeftSymmetric), 4, 1);
        assert_eq!(row.q_exponent(0, 4), 0);
        assert_eq!(row.q_exponent(1, 4), 1);
        // DDF numbers data columns by column index.
        let row = RowPlacement::new(ParityKind::Raid6(Raid6Layout::RotatingNContinue), 4, 1);
        assert_eq!(row.data, vec![3, 0]);
        assert_eq!(row.q_exponent(3, 4), 3);
        assert_eq!(row.q_exponent(0, 4), 0);
    }

    #[test]
    fn layout_codes_round_trip() {
        for raw in 0..32 {
            if let Some(layout) = Raid5Layout::from_raw(raw) {
                assert_eq!(layout.raw(), raw);
            }
            if let Some(layout) = Raid6Layout::from_raw(raw) {
                assert_eq!(layout.raw(), raw);
            }
        }
        assert_eq!(Raid5Layout::from_raw(6), None);
        assert_eq!(Raid6Layout::from_raw(7), None);
        assert_eq!(Raid6Layout::from_raw(21), None);
    }
}
