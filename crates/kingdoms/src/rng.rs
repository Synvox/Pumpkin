//! Deterministic splitmix64 used for every town decision.

/// Seeded RNG. Same seed always yields the same stream.
#[derive(Clone, Debug)]
pub struct TownRng {
    state: u64,
}

impl TownRng {
    /// Creates a generator from `seed`.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Mixes extra salts into a new generator.
    #[must_use]
    pub const fn salted(seed: u64, a: u64, b: u64) -> Self {
        Self {
            state: seed ^ a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.rotate_left(17),
        }
    }

    /// Next raw 64-bit value.
    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Integer in `[min, max)`.
    pub fn gen_range(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        let span = (max - min) as u64;
        min + (self.next_u64() % span) as i32
    }

    /// `true` with probability `n/d`.
    pub fn chance(&mut self, n: u32, d: u32) -> bool {
        if d == 0 {
            return false;
        }
        (self.next_u64() % u64::from(d)) < u64::from(n)
    }

    /// Picks one of `items`. Empty slice yields `None`.
    pub fn choose<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }
        let index = (self.next_u64() as usize) % items.len();
        items.get(index)
    }
}
