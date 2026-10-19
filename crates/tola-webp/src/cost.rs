//! Fixed-point bit costs for RD decisions.
//!
//! Costs are in 1/256 bit so every comparison is integer and identical on every target.

/// The cost of coding a bool whose less-probable branch has probability `q / 256`.
const BOOL_COST: [u16; 257] = [
    0, 2048, 1792, 1642, 1536, 1454, 1386, 1329, 1280, 1236, 1198, 1162, 1130, 1101, 1073, 1048,
    1024, 1002, 980, 961, 942, 924, 906, 890, 874, 859, 845, 831, 817, 804, 792, 780, 768, 757,
    746, 735, 724, 714, 705, 695, 686, 676, 668, 659, 650, 642, 634, 626, 618, 611, 603, 596, 589,
    582, 575, 568, 561, 555, 548, 542, 536, 530, 524, 518, 512, 506, 501, 495, 490, 484, 479, 474,
    468, 463, 458, 453, 449, 444, 439, 434, 430, 425, 420, 416, 412, 407, 403, 399, 394, 390, 386,
    382, 378, 374, 370, 366, 362, 358, 355, 351, 347, 343, 340, 336, 333, 329, 326, 322, 319, 315,
    312, 309, 305, 302, 299, 296, 292, 289, 286, 283, 280, 277, 274, 271, 268, 265, 262, 259, 256,
    253, 250, 247, 245, 242, 239, 236, 234, 231, 228, 226, 223, 220, 218, 215, 212, 210, 207, 205,
    202, 200, 197, 195, 193, 190, 188, 185, 183, 181, 178, 176, 174, 171, 169, 167, 164, 162, 160,
    158, 156, 153, 151, 149, 147, 145, 143, 140, 138, 136, 134, 132, 130, 128, 126, 124, 122, 120,
    118, 116, 114, 112, 110, 108, 106, 104, 102, 101, 99, 97, 95, 93, 91, 89, 87, 86, 84, 82, 80,
    78, 77, 75, 73, 71, 70, 68, 66, 64, 63, 61, 59, 58, 56, 54, 53, 51, 49, 48, 46, 44, 43, 41, 40,
    38, 36, 35, 33, 32, 30, 28, 27, 25, 24, 22, 21, 19, 18, 16, 15, 13, 12, 10, 9, 7, 6, 4, 3, 1,
    0,
];

/// The cost, in 1/256 bit, of coding `value` with a probability `probability` of false.
pub(crate) const fn bool_cost(probability: u8, value: bool) -> u32 {
    let zero = probability as u32;
    let branch = if value { 256 - zero } else { zero };
    BOOL_COST[branch as usize] as u32
}

/// Accumulates the cost of the bools a candidate would write.
pub(crate) struct CostWriter {
    bits: u64,
}

impl CostWriter {
    pub(crate) fn new() -> Self {
        Self { bits: 0 }
    }

    pub(crate) fn bits(&self) -> u64 {
        self.bits
    }

    pub(crate) fn write_bool(&mut self, probability: u8, value: bool) {
        let zero = u32::from(probability);
        let branch = if value { 256 - zero } else { zero };
        self.bits += u64::from(BOOL_COST[branch as usize]);
    }
}
