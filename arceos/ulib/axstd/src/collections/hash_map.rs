//! A minimal `HashMap` implementation for the no_std `axstd` environment.
//!
//! Uses separate chaining and an FNV-1a 64-bit hasher seeded by the
//! kernel's `random()` source so different runs produce different hash
//! orderings (matches `std::collections::HashMap`'s non-deterministic
//! iteration order semantics, while remaining DoS-resistant in practice).

use core::borrow::Borrow;
use core::hash::{BuildHasher, Hash, Hasher};
use core::iter::FromIterator;
use core::mem;

use alloc::vec::Vec;

use arceos_api::modules::axhal;

const INITIAL_BUCKETS: usize = 16;

/// FNV-1a 64-bit hasher.
pub struct FnvHasher {
    state: u64,
}

impl FnvHasher {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x100_0000_01b3;
}

impl Hasher for FnvHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut state = self.state;
        for &b in bytes {
            state ^= b as u64;
            state = state.wrapping_mul(Self::FNV_PRIME);
        }
        self.state = state;
    }

    fn finish(&self) -> u64 {
        self.state
    }
}

/// A `BuildHasher` that produces `FnvHasher`s seeded with a per-map random key.
#[derive(Clone, Copy)]
pub struct RandomState {
    seed: u64,
}

impl RandomState {
    fn new() -> Self {
        let r = axhal::misc::random();
        let seed = (r as u64) ^ ((r >> 64) as u64) ^ FnvHasher::FNV_OFFSET;
        Self { seed }
    }
}

impl Default for RandomState {
    fn default() -> Self {
        Self::new()
    }
}

impl BuildHasher for RandomState {
    type Hasher = FnvHasher;

    fn build_hasher(&self) -> FnvHasher {
        FnvHasher { state: self.seed }
    }
}

/// A hash map keyed by `K`, holding values of type `V`.
pub struct HashMap<K, V, S = RandomState> {
    buckets: Vec<Vec<(K, V)>>,
    len: usize,
    hasher: S,
}

impl<K: Hash + Eq, V> HashMap<K, V, RandomState> {
    /// Creates an empty `HashMap`.
    pub fn new() -> Self {
        Self::with_hasher(RandomState::new())
    }
}

impl<K: Hash + Eq, V> Default for HashMap<K, V, RandomState> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, S> HashMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    /// Creates an empty `HashMap` with the given hasher state.
    pub fn with_hasher(hasher: S) -> Self {
        let mut buckets = Vec::with_capacity(INITIAL_BUCKETS);
        buckets.resize_with(INITIAL_BUCKETS, Vec::new);
        Self {
            buckets,
            len: 0,
            hasher,
        }
    }

    /// Returns the number of elements in the map.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the map contains no elements.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn bucket_index<Q>(&self, key: &Q, n_buckets: usize) -> usize
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash,
    {
        let mut h = self.hasher.build_hasher();
        key.hash(&mut h);
        (h.finish() as usize) & (n_buckets - 1)
    }

    /// Inserts a key-value pair. Returns the previous value if any.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if self.len * 4 >= self.buckets.len() * 3 {
            self.resize();
        }
        let n = self.buckets.len();
        let idx = self.bucket_index(&key, n);
        let bucket = &mut self.buckets[idx];
        for &mut (ref k, ref mut v) in bucket.iter_mut() {
            if k == &key {
                return Some(mem::replace(v, value));
            }
        }
        bucket.push((key, value));
        self.len += 1;
        None
    }

    /// Returns a reference to the value corresponding to the key.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let idx = self.bucket_index(key, self.buckets.len());
        self.buckets[idx]
            .iter()
            .find(|(k, _)| k.borrow() == key)
            .map(|(_, v)| v)
    }

    /// Returns `true` if the map contains a value for the specified key.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.get(key).is_some()
    }

    /// Removes a key from the map, returning the value if it existed.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let idx = self.bucket_index(key, self.buckets.len());
        let bucket = &mut self.buckets[idx];
        let pos = bucket.iter().position(|(k, _)| k.borrow() == key)?;
        let (_, v) = bucket.swap_remove(pos);
        self.len -= 1;
        Some(v)
    }

    /// An iterator visiting all key-value pairs in arbitrary order.
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            buckets: self.buckets.iter(),
            cur: None,
        }
    }

    fn resize(&mut self) {
        let new_n = self.buckets.len() * 2;
        let mut new_buckets: Vec<Vec<(K, V)>> = Vec::with_capacity(new_n);
        new_buckets.resize_with(new_n, Vec::new);
        for bucket in self.buckets.drain(..) {
            for (k, v) in bucket {
                let mut h = self.hasher.build_hasher();
                k.hash(&mut h);
                let idx = (h.finish() as usize) & (new_n - 1);
                new_buckets[idx].push((k, v));
            }
        }
        self.buckets = new_buckets;
    }
}

/// Iterator over `&(K, V)` entries of a [`HashMap`].
pub struct Iter<'a, K, V> {
    buckets: core::slice::Iter<'a, Vec<(K, V)>>,
    cur: Option<core::slice::Iter<'a, (K, V)>>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(it) = self.cur.as_mut() {
                if let Some((k, v)) = it.next() {
                    return Some((k, v));
                }
            }
            self.cur = Some(self.buckets.next()?.iter());
        }
    }
}

impl<'a, K, V, S> IntoIterator for &'a HashMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<K, V> FromIterator<(K, V)> for HashMap<K, V, RandomState>
where
    K: Hash + Eq,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut m = Self::new();
        for (k, v) in iter {
            m.insert(k, v);
        }
        m
    }
}
