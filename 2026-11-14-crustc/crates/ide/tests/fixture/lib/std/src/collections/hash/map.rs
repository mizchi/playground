/// A hash map.
pub struct HashMap<K, V> {
    base: RawTable<(K, V)>,
}

impl<K, V> HashMap<K, V> {
    pub fn new() -> HashMap<K, V> {
        loop {}
    }
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        loop {}
    }
    pub fn get(&self, k: &K) -> Option<&V> {
        loop {}
    }
    pub fn len(&self) -> usize {
        loop {}
    }
}
