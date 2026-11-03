//! Column-oriented storage of base cells.

use hashbrown::HashTable;
use roaring::RoaringBitmap;
use std::collections::HashMap;

/// Base cells of one cube. A cell is addressed by the DFS ordinal of its base member per dimension.
pub struct FactStore {
    n_dims: usize,
    /// One column per dimension with the leaf ordinal of each row.
    cols: Vec<Vec<u32>>,
    values: Vec<f64>,
    live: Vec<bool>,
    free_rows: Vec<u32>,
    n_live: usize,
    /// Hash over the coordinates → row (no copy of the key).
    key_index: HashTable<u32>,
    /// Per dimension: base member → rows.
    leaf_rows: Vec<HashMap<u32, RoaringBitmap>>,
}

fn hash_coords(coords: impl Iterator<Item = u32>) -> u64 {
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15;
    for c in coords {
        h ^= u64::from(c)
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(h << 6)
            .wrapping_add(h >> 2);
        h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        h ^= h >> 32;
    }
    h
}

impl FactStore {
    pub fn new(n_dims: usize) -> Self {
        FactStore {
            n_dims,
            cols: vec![Vec::new(); n_dims],
            values: Vec::new(),
            live: Vec::new(),
            free_rows: Vec::new(),
            n_live: 0,
            key_index: HashTable::new(),
            leaf_rows: vec![HashMap::new(); n_dims],
        }
    }

    pub fn n_dims(&self) -> usize {
        self.n_dims
    }

    /// Number of filled cells.
    pub fn len(&self) -> usize {
        self.n_live
    }

    pub fn is_empty(&self) -> bool {
        self.n_live == 0
    }

    /// Number of row slots (live + free); upper bound for row numbers.
    pub fn n_rows(&self) -> usize {
        self.values.len()
    }

    pub fn is_live(&self, row: usize) -> bool {
        self.live[row]
    }

    pub fn value(&self, row: usize) -> f64 {
        self.values[row]
    }

    pub fn col(&self, dim: usize) -> &[u32] {
        &self.cols[dim]
    }

    pub fn rows_of_leaf(&self, dim: usize, ord: u32) -> Option<&RoaringBitmap> {
        self.leaf_rows[dim].get(&ord)
    }

    fn find(&self, coords: &[u32]) -> Option<u32> {
        let h = hash_coords(coords.iter().copied());
        self.key_index
            .find(h, |&row| {
                (0..self.n_dims).all(|d| self.cols[d][row as usize] == coords[d])
            })
            .copied()
    }

    pub fn get(&self, coords: &[u32]) -> Option<f64> {
        self.find(coords).map(|r| self.values[r as usize])
    }

    /// Sets a cell; returns the previous value.
    pub fn set(&mut self, coords: &[u32], value: f64) -> Option<f64> {
        debug_assert_eq!(coords.len(), self.n_dims);
        if let Some(row) = self.find(coords) {
            return Some(std::mem::replace(&mut self.values[row as usize], value));
        }
        let row = match self.free_rows.pop() {
            Some(r) => {
                for (d, &c) in coords.iter().enumerate() {
                    self.cols[d][r as usize] = c;
                }
                self.values[r as usize] = value;
                self.live[r as usize] = true;
                r
            }
            None => {
                for (d, &c) in coords.iter().enumerate() {
                    self.cols[d].push(c);
                }
                self.values.push(value);
                self.live.push(true);
                (self.values.len() - 1) as u32
            }
        };
        for (d, &c) in coords.iter().enumerate() {
            self.leaf_rows[d].entry(c).or_default().insert(row);
        }
        let cols = &self.cols;
        let n = self.n_dims;
        self.key_index
            .insert_unique(hash_coords(coords.iter().copied()), row, |&r| {
                hash_coords((0..n).map(|d| cols[d][r as usize]))
            });
        self.n_live += 1;
        None
    }

    /// Deletes a cell; returns its value.
    pub fn remove(&mut self, coords: &[u32]) -> Option<f64> {
        let row = self.find(coords)?;
        let h = hash_coords(coords.iter().copied());
        if let Ok(e) = self.key_index.find_entry(h, |&r| r == row) {
            e.remove();
        }
        for (d, &c) in coords.iter().enumerate() {
            if let Some(b) = self.leaf_rows[d].get_mut(&c) {
                b.remove(row);
                if b.is_empty() {
                    self.leaf_rows[d].remove(&c);
                }
            }
        }
        self.live[row as usize] = false;
        self.free_rows.push(row);
        self.n_live -= 1;
        Some(std::mem::take(&mut self.values[row as usize]))
    }

    /// Live cells as `(coordinates, value)`.
    pub fn iter(&self) -> impl Iterator<Item = (Vec<u32>, f64)> + '_ {
        (0..self.n_rows()).filter(|&r| self.live[r]).map(|r| {
            (
                (0..self.n_dims).map(|d| self.cols[d][r]).collect(),
                self.values[r],
            )
        })
    }
}
