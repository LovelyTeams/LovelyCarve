// Divide and yield remainder.
pub fn div_rem(a: u64, b: u64) -> (u64, u64) {
    (a / b, a % b)
}

// Divide, rounding up.
pub fn div_ceil(a: u64, b: u64) -> u64 {
    let (d, m) = div_rem(a, b);
    if m == 0 {
        d
    } else {
        d + 1
    }
}
