//! Deterministic fuzz smoke-test helpers shared by the codec tests: an
//! xorshift64* PRNG and a byte-string mutator.

#![cfg(test)]

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform-ish in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub fn byte(&mut self) -> u8 {
        self.next_u64() as u8
    }

    /// A mutated copy of a randomly chosen seed.
    pub fn mutate_any(&mut self, seeds: &[Vec<u8>]) -> Vec<u8> {
        let i = self.below(seeds.len());
        self.mutate(&seeds[i])
    }

    /// A copy of `seed` with 1..=8 random edits: bit flips, byte overwrites
    /// (biased toward boundary values), insertions, deletions, truncation,
    /// and duplicated slices.
    pub fn mutate(&mut self, seed: &[u8]) -> Vec<u8> {
        let mut v = seed.to_vec();
        let edits = 1 + self.below(8);
        for _ in 0..edits {
            let len = v.len();
            match self.below(7) {
                0 if len > 0 => {
                    let i = self.below(len);
                    v[i] ^= 1 << self.below(8);
                }
                1 if len > 0 => {
                    let i = self.below(len);
                    const SPECIAL: [u8; 8] = [0x00, 0x01, 0x7f, 0x80, 0xff, 0xfe, 0x0f, 0x10];
                    v[i] = if self.below(2) == 0 {
                        SPECIAL[self.below(SPECIAL.len())]
                    } else {
                        self.byte()
                    };
                }
                2 => {
                    let i = self.below(len + 1);
                    let b = self.byte();
                    v.insert(i, b);
                }
                3 if len > 0 => {
                    let i = self.below(len);
                    v.remove(i);
                }
                4 if len > 0 => {
                    let n = self.below(len);
                    v.truncate(n);
                }
                5 if len > 0 => {
                    let a = self.below(len);
                    let b = a + self.below(len - a).min(64);
                    let chunk = v[a..b].to_vec();
                    let at = self.below(v.len() + 1);
                    v.splice(at..at, chunk);
                }
                _ => {
                    let b = self.byte();
                    v.push(b);
                }
            }
        }
        v
    }
}
