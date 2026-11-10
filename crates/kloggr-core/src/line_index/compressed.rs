//! Compressed storage for increasing file offsets.
//!
//! Port of klogg's `CompressedLinePositionStorage`
//! (`src/logdata/src/compressedlinestorage.cpp`, Copyright (C) Nicolas Bonnefon
//! and other klogg contributors, GPLv3). klogg packs blocks of 128 offsets with
//! SIMD stream-vbyte, relative to the block's first offset as `u32`, which fails
//! when one block spans more than 4 GiB. Here each block stores its first offset
//! as `u64` and the following ones as LEB128 varint deltas (u64), so any gap
//! fits. A typical log line (< 128 bytes) costs one byte instead of eight.
//!
//! Lookup is O(1) in blocks plus decoding at most 127 varints, which is cheap
//! next to rendering a line.

use std::sync::Arc;

/// Number of offsets per compressed block (klogg: `SimdIndexBlockSize`).
const BLOCK: usize = 128;
/// Blocks per segment: 512 Ki offsets, about 0.5–1 MiB packed.
const SEGMENT_BLOCKS: usize = 4096;

#[derive(Debug, Clone, Copy)]
struct BlockMeta {
    first: u64,
    /// Start of this block's deltas in its segment's `packed`.
    packed_at: usize,
}

/// A run of [`SEGMENT_BLOCKS`] compressed blocks.
#[derive(Debug, Default, Clone)]
struct Segment {
    metas: Vec<BlockMeta>,
    /// Varint deltas of the segment's blocks, 127 per block.
    packed: Vec<u8>,
}

impl Segment {
    fn allocated_bytes(&self) -> usize {
        self.packed.capacity() + self.metas.capacity() * size_of::<BlockMeta>()
    }
}

/// Append-only, strictly increasing sequence of `u64`, stored compressed.
///
/// Full segments are shared (`Arc`), so a clone costs one pointer per
/// 512 Ki offsets plus the open segment: follow mode clones the index of a
/// growing file on every update instead of copying hundreds of MB.
#[derive(Debug, Default, Clone)]
pub struct CompressedOffsets {
    sealed: Vec<Arc<Segment>>,
    /// The segment being filled.
    open: Segment,
    /// The last, not yet full block, uncompressed.
    tail: Vec<u64>,
}

impl CompressedOffsets {
    pub fn new() -> Self {
        Self::default()
    }

    fn block_count(&self) -> usize {
        self.sealed.len() * SEGMENT_BLOCKS + self.open.metas.len()
    }

    pub fn len(&self) -> usize {
        self.block_count() * BLOCK + self.tail.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn last(&self) -> Option<u64> {
        if let Some(&v) = self.tail.last() {
            return Some(v);
        }
        let block = self.block_count().checked_sub(1)?;
        Some(self.decode_block(block)[BLOCK - 1])
    }

    /// Appends `value`, which must be greater than the last value.
    pub fn push(&mut self, value: u64) {
        debug_assert!(
            self.last().is_none_or(|last| value > last),
            "offsets must be strictly increasing"
        );
        self.tail.push(value);
        if self.tail.len() == BLOCK {
            self.compress_tail();
        }
    }

    pub fn extend_from_slice(&mut self, values: &[u64]) {
        for &v in values {
            self.push(v);
        }
    }

    /// The segment holding compressed block `block`, and the block's index
    /// in it.
    fn segment(&self, block: usize) -> (&Segment, usize) {
        let (s, at) = (block / SEGMENT_BLOCKS, block % SEGMENT_BLOCKS);
        match self.sealed.get(s) {
            Some(segment) => (segment, at),
            None => (&self.open, at),
        }
    }

    pub fn get(&self, i: usize) -> Option<u64> {
        let (block, at) = (i / BLOCK, i % BLOCK);
        if block == self.block_count() {
            return self.tail.get(at).copied();
        }
        if block > self.block_count() {
            return None;
        }
        let (segment, b) = self.segment(block);
        let meta = segment.metas[b];
        let mut value = meta.first;
        let mut pos = meta.packed_at;
        for _ in 0..at {
            value += read_varint(&segment.packed, &mut pos);
        }
        Some(value)
    }

    /// Values at indices `range`, decoding each block once.
    ///
    /// # Panics
    /// If the range is out of bounds.
    pub fn range(&self, range: std::ops::Range<usize>) -> Vec<u64> {
        assert!(range.end <= self.len(), "range out of bounds");
        let blocks = self.block_count();
        let mut out = Vec::with_capacity(range.len());
        let mut i = range.start;
        while i < range.end {
            let (block, at) = (i / BLOCK, i % BLOCK);
            let take = (BLOCK - at).min(range.end - i);
            if block == blocks {
                out.extend_from_slice(&self.tail[at..at + take]);
            } else {
                out.extend_from_slice(&self.decode_block(block)[at..at + take]);
            }
            i += take;
        }
        out
    }

    /// Removes values from index `len` onwards (klogg: repeated `pop_back`).
    pub fn truncate(&mut self, len: usize) {
        if len >= self.len() {
            return;
        }
        let keep_blocks = len / BLOCK;
        if keep_blocks < self.block_count() {
            let s = keep_blocks / SEGMENT_BLOCKS;
            if s < self.sealed.len() {
                // Reopen the segment that will hold the new tail.
                self.open = (*self.sealed[s]).clone();
                self.sealed.truncate(s);
            }
            let local = keep_blocks - self.sealed.len() * SEGMENT_BLOCKS;
            self.tail = self.decode_block(keep_blocks).to_vec();
            self.open.packed.truncate(self.open.metas[local].packed_at);
            self.open.metas.truncate(local);
        }
        self.tail.truncate(len % BLOCK);
    }

    /// Releases spare capacity left by growth. Called when indexing finishes:
    /// on a multi-GB file the spare capacity can nearly double the index.
    pub fn shrink_to_fit(&mut self) {
        self.open.packed.shrink_to_fit();
        self.open.metas.shrink_to_fit();
        self.sealed.shrink_to_fit();
    }

    /// Heap bytes used (klogg: `allocatedSize`). Shared segments count in
    /// full for every clone.
    pub fn allocated_bytes(&self) -> usize {
        self.sealed
            .iter()
            .map(|s| s.allocated_bytes())
            .sum::<usize>()
            + self.open.allocated_bytes()
            + self.sealed.capacity() * size_of::<Arc<Segment>>()
            + self.tail.capacity() * size_of::<u64>()
    }

    fn compress_tail(&mut self) {
        let packed_at = self.open.packed.len();
        self.open.metas.push(BlockMeta {
            first: self.tail[0],
            packed_at,
        });
        for pair in self.tail.windows(2) {
            write_varint(&mut self.open.packed, pair[1] - pair[0]);
        }
        self.tail.clear();
        if self.open.metas.len() == SEGMENT_BLOCKS {
            let mut full = std::mem::take(&mut self.open);
            full.packed.shrink_to_fit();
            full.metas.shrink_to_fit();
            self.sealed.push(Arc::new(full));
        }
    }

    fn decode_block(&self, block: usize) -> [u64; BLOCK] {
        let (segment, b) = self.segment(block);
        let meta = segment.metas[b];
        let mut out = [0; BLOCK];
        out[0] = meta.first;
        let mut pos = meta.packed_at;
        for k in 1..BLOCK {
            out[k] = out[k - 1] + read_varint(&segment.packed, &mut pos);
        }
        out
    }
}

fn write_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push(v as u8 | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn read_varint(buf: &[u8], pos: &mut usize) -> u64 {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = buf[*pos];
        *pos += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b < 0x80 {
            return v;
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> Vec<u64> {
        // Mix of small, large and > 4 GiB gaps (the klogg u32 limit).
        let mut v = 0u64;
        (0..n)
            .map(|i| {
                v += match i % 7 {
                    0 => 1,
                    3 => 300,
                    5 => 5 << 32,
                    _ => 80,
                };
                v
            })
            .collect()
    }

    fn stored(values: &[u64]) -> CompressedOffsets {
        let mut c = CompressedOffsets::new();
        c.extend_from_slice(values);
        c
    }

    #[test]
    fn get_matches_input() {
        for n in [0, 1, 127, 128, 129, 1000] {
            let values = sample(n);
            let c = stored(&values);
            assert_eq!(c.len(), n);
            for (i, &v) in values.iter().enumerate() {
                assert_eq!(c.get(i), Some(v), "n={n} i={i}");
            }
            assert_eq!(c.get(n), None);
            assert_eq!(c.last(), values.last().copied());
        }
    }

    #[test]
    fn range_matches_input() {
        let values = sample(1000);
        let c = stored(&values);
        for r in [
            0..0,
            0..1000,
            5..6,
            120..300,
            127..129,
            896..1000,
            990..1000,
        ] {
            assert_eq!(c.range(r.clone()), &values[r.clone()], "{r:?}");
        }
    }

    #[test]
    fn truncate_then_push() {
        let values = sample(600);
        for keep in [0, 1, 127, 128, 129, 256, 599, 600, 700] {
            let mut c = stored(&values);
            c.truncate(keep);
            let keep = keep.min(values.len());
            assert_eq!(c.range(0..c.len()), &values[..keep]);
            c.extend_from_slice(&values[keep..]);
            assert_eq!(c.range(0..c.len()), values, "keep={keep}");
        }
    }

    #[test]
    fn segments_share_on_clone_and_truncate() {
        let n = BLOCK * SEGMENT_BLOCKS * 2 + 1000;
        let values: Vec<u64> = (0..n as u64).map(|i| i * 3 + (i % 2)).collect();
        let c = stored(&values);
        assert_eq!(c.sealed.len(), 2);
        let mut copy = c.clone();
        assert!(Arc::ptr_eq(&c.sealed[0], &copy.sealed[0]));
        for i in [0, BLOCK * SEGMENT_BLOCKS - 1, BLOCK * SEGMENT_BLOCKS, n - 1] {
            assert_eq!(copy.get(i), Some(values[i]), "i={i}");
        }
        // Truncating into a sealed segment reopens it; the original keeps it.
        let keep = BLOCK * SEGMENT_BLOCKS + 300;
        copy.truncate(keep);
        assert_eq!(copy.len(), keep);
        assert_eq!(copy.last(), Some(values[keep - 1]));
        copy.extend_from_slice(&values[keep..]);
        assert_eq!(copy.range(0..n), values);
        assert_eq!(c.range(0..n), values);
    }

    #[test]
    fn compresses_typical_lines() {
        let values: Vec<u64> = (0..100_000u64).map(|i| i * 90).collect();
        let c = stored(&values);
        assert!(
            c.allocated_bytes() < values.len() * 2,
            "{}",
            c.allocated_bytes()
        );
    }
}
