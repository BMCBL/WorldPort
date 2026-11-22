//! Compares palette bit-packing kernels.
//!
//! Every chunk section packs 4096 palette indices into 64-bit words, so a
//! 108k-chunk world packs ~1.5M sections. The shipped kernel fills one word at
//! a time from its lanes (`grouped`); the previous one read-modify-wrote a
//! scattered word per index (`scattered`).
//!
//! An AVX2 variant was measured and rejected: the per-word horizontal
//! reduction costs more than the scalar lane loop it replaces (0.07-0.09x the
//! grouped speed), so no SIMD kernel ships here.

use std::time::Instant;

fn pack_scattered(indices: &[u16], bits: u32, words: &mut [i64]) {
    let per_long = (64 / bits) as usize;
    let mask = (1_u64 << bits) - 1;
    for word in words.iter_mut() {
        *word = 0;
    }
    for (index, value) in indices.iter().enumerate() {
        words[index / per_long] |=
            ((u64::from(*value) & mask) << ((index % per_long) as u32 * bits)) as i64;
    }
}

fn pack_grouped(indices: &[u16], bits: u32, words: &mut [i64]) {
    let per_long = (64 / bits) as usize;
    let mask = (1_u64 << bits) - 1;
    for (word_index, word) in words.iter_mut().enumerate() {
        let base = word_index * per_long;
        let mut packed = 0_u64;
        for lane in 0..per_long {
            let Some(value) = indices.get(base + lane) else {
                break;
            };
            packed |= (u64::from(*value) & mask) << (lane as u32 * bits);
        }
        *word = packed as i64;
    }
}

fn main() {
    let iterations = 20_000;
    for bits in [4_u32, 5, 6, 7, 8, 9] {
        let per_long = (64 / bits) as usize;
        let indices: Vec<u16> = (0..4096)
            .map(|index| u16::try_from(index % (1usize << bits)).unwrap_or(0))
            .collect();
        let mut scattered_words = vec![0_i64; 4096_usize.div_ceil(per_long)];
        let mut grouped_words = vec![0_i64; 4096_usize.div_ceil(per_long)];

        let started = Instant::now();
        for _ in 0..iterations {
            pack_scattered(&indices, bits, &mut scattered_words);
        }
        let scattered = started.elapsed();

        let started = Instant::now();
        for _ in 0..iterations {
            pack_grouped(&indices, bits, &mut grouped_words);
        }
        let grouped = started.elapsed();

        assert_eq!(scattered_words, grouped_words, "kernels must agree");
        println!(
            "bits={bits}: scattered {:>8.2?}  grouped {:>8.2?}  ({:.2}x)",
            scattered,
            grouped,
            scattered.as_secs_f64() / grouped.as_secs_f64()
        );
    }
}
