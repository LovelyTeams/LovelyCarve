use nom::combinator::all_consuming;
use nom::multi::fold_many0;
use nom::number::complete::{le_u32, le_u64};
use nom::sequence::pair;
use nom::IResult;

#[derive(Debug, Clone)]
pub struct Fletcher4 {
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub d: u64,
}

impl Fletcher4 {
    pub fn new() -> Self {
        Self {
            a: 0,
            b: 0,
            c: 0,
            d: 0,
        }
    }
    fn input(&mut self, data: u32) {
        self.a = self.a.wrapping_add(data as u64);
        self.b = self.b.wrapping_add(self.a);
        self.c = self.c.wrapping_add(self.b);
        self.d = self.d.wrapping_add(self.c);
    }
    /// Fails if input length is not a multiple of 4
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        all_consuming(fold_many0(le_u32, Self::new(), |mut cksum, input| {
            cksum.input(input);
            cksum
        }))(input)
    }
}

#[derive(Debug, Clone)]
pub struct Fletcher2 {
    pub a0: u64,
    pub a1: u64,
    pub b0: u64,
    pub b1: u64,
}

impl Fletcher2 {
    pub fn new() -> Self {
        Self {
            a0: 0,
            a1: 0,
            b0: 0,
            b1: 0,
        }
    }
    fn input(&mut self, data0: u64, data1: u64) {
        self.a0 = self.a0.wrapping_add(data0);
        self.a1 = self.a1.wrapping_add(data1);
        self.b0 = self.b0.wrapping_add(self.a0);
        self.b1 = self.b1.wrapping_add(self.a1);
    }
    /// Fails if input length is not a multiple of 16
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        all_consuming(fold_many0(
            pair(le_u64, le_u64),
            Self::new(),
            |mut cksum, (input0, input1)| {
                cksum.input(input0, input1);
                cksum
            },
        ))(input)
    }
}
