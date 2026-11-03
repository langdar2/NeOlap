//! Column-oriented storage of base cells.

use hashbrown::HashTable;

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
    /// Per dimension: number of live rows per base ordinal (selectivity estimates).
    counts: Vec<Vec<u32>>,
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
            counts: vec![Vec::new(); n_dims],
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

    /// Number of filled cells with the given base member in dimension `dim`.
    pub fn count_of_leaf(&self, dim: usize, ord: u32) -> usize {
        self.counts[dim]
            .get(ord as usize)
            .map_or(0, |&c| c as usize)
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
                if self.values.len() == self.values.capacity() {
                    // Grow by 1/8 instead of doubling: at most 12.5 % slack in the columns.
                    let extra = (self.values.len() / 8).max(1024);
                    self.cols.iter_mut().for_each(|c| c.reserve_exact(extra));
                    self.values.reserve_exact(extra);
                    self.live.reserve_exact(extra);
                }
                for (d, &c) in coords.iter().enumerate() {
                    self.cols[d].push(c);
                }
                self.values.push(value);
                self.live.push(true);
                (self.values.len() - 1) as u32
            }
        };
        for (d, &c) in coords.iter().enumerate() {
            let counts = &mut self.counts[d];
            if counts.len() <= c as usize {
                counts.resize(c as usize + 1, 0);
            }
            counts[c as usize] += 1;
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
            self.counts[d][c as usize] -= 1;
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

/// Memory use per component in bytes (from capacities).
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryStats {
    pub columns: usize,
    pub values: usize,
    pub hash_index: usize,
    pub counts: usize,
}

impl FactStore {
    pub fn memory_stats(&self) -> MemoryStats {
        MemoryStats {
            columns: self.cols.iter().map(|c| c.capacity() * 4).sum(),
            values: self.values.capacity() * 8 + self.live.capacity(),
            hash_index: self.key_index.capacity() * 5,
            counts: self.counts.iter().map(|c| c.capacity() * 4).sum(),
        }
    }
}
