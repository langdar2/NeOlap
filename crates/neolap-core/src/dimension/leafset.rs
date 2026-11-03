//! Base-member sets of aggregates (`LeafSet`).

use std::ops::Range;

/// Shape of a leaf set: a contiguous range of DFS ordinals (the normal case for pure trees)
/// or a bit set over all base members (alternative hierarchies).
#[derive(Debug, Clone, PartialEq)]
pub enum LeafShape {
    Range(Range<u32>),
    Bits { words: Vec<u64>, count: u32 },
}

/// The base members below an aggregate, expressed as DFS ordinals, plus the path weights of
/// those base members whose weight differs from 1.
#[derive(Debug, Clone, PartialEq)]
pub struct LeafSet {
    shape: LeafShape,
    /// Sorted by ordinal; only entries with weight ≠ 1.
    weights: Vec<(u32, f32)>,
}

impl LeafSet {
    /// Builds a leaf set from `(ordinal, weight)` pairs (unique ordinals, any order).
    pub fn from_leaves(mut leaves: Vec<(u32, f32)>, n_leaves: u32) -> Self {
        leaves.sort_unstable_by_key(|l| l.0);
        let weights: Vec<(u32, f32)> = leaves.iter().copied().filter(|l| l.1 != 1.0).collect();
        let shape = match (leaves.first(), leaves.last()) {
            (None, _) | (_, None) => LeafShape::Range(0..0),
            (Some(first), Some(last)) if (last.0 - first.0) as usize + 1 == leaves.len() => {
                LeafShape::Range(first.0..last.0 + 1)
            }
            _ => {
                let mut words = vec![0u64; (n_leaves as usize).div_ceil(64)];
                for l in &leaves {
                    words[(l.0 / 64) as usize] |= 1 << (l.0 % 64);
                }
                LeafShape::Bits {
                    words,
                    count: leaves.len() as u32,
                }
            }
        };
        LeafSet { shape, weights }
    }

    pub fn shape(&self) -> &LeafShape {
        &self.shape
    }

    /// Number of base members.
    pub fn len(&self) -> usize {
        match &self.shape {
            LeafShape::Range(r) => r.len(),
            LeafShape::Bits { count, .. } => *count as usize,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains(&self, ord: u32) -> bool {
        match &self.shape {
            LeafShape::Range(r) => r.contains(&ord),
            LeafShape::Bits { words, .. } => words
                .get((ord / 64) as usize)
                .is_some_and(|w| w & (1 << (ord % 64)) != 0),
        }
    }

    /// Path weight of a base member (1 unless listed in the weight table).
    pub fn weight(&self, ord: u32) -> f32 {
        match self.weights.binary_search_by_key(&ord, |w| w.0) {
            Ok(i) => self.weights[i].1,
            Err(_) => 1.0,
        }
    }

    pub fn has_weights(&self) -> bool {
        !self.weights.is_empty()
    }

    /// The contiguous ordinal range, if the set has that shape.
    pub fn as_range(&self) -> Option<Range<u32>> {
        match &self.shape {
            LeafShape::Range(r) => Some(r.clone()),
            LeafShape::Bits { .. } => None,
        }
    }

    /// Ordinals in ascending order.
    pub fn iter(&self) -> Box<dyn Iterator<Item = u32> + '_> {
        match &self.shape {
            LeafShape::Range(r) => Box::new(r.clone()),
            LeafShape::Bits { words, .. } => {
                Box::new(words.iter().enumerate().flat_map(|(i, &w)| {
                    let mut w = w;
                    std::iter::from_fn(move || {
                        if w == 0 {
                            return None;
                        }
                        let b = w.trailing_zeros();
                        w &= w - 1;
                        Some(i as u32 * 64 + b)
                    })
                }))
            }
        }
    }
}
