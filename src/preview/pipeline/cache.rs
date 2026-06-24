use std::collections::HashMap;

use crate::preview::pipeline::params::PreviewParams;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub color_space: u32,
    pub transfer: u32,
    pub adjust_enabled: u32,
}

impl PipelineKey {
    pub fn from_params(params: &PreviewParams) -> Self {
        Self {
            color_space: params.color_space,
            transfer: params.transfer,
            adjust_enabled: params.adjust_enabled,
        }
    }
}

pub struct PipelineCache {
    entries: HashMap<PipelineKey, wgpu::ComputePipeline>,
    pub(crate) access_order: Vec<PipelineKey>,
    pub(crate) max_entries: usize,
}

impl PipelineCache {
    pub const MAX_ENTRIES: usize = 20;

    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            access_order: Vec::new(),
            max_entries: Self::MAX_ENTRIES,
        }
    }

    pub fn get(&mut self, key: &PipelineKey) -> Option<&wgpu::ComputePipeline> {
        if self.entries.contains_key(key) {
            if let Some(pos) = self.access_order.iter().position(|k| k == key) {
                self.access_order.remove(pos);
            }
            self.access_order.push(*key);
            return self.entries.get(key);
        }
        None
    }

    pub fn insert(&mut self, key: PipelineKey, pipeline: wgpu::ComputePipeline) {
        if self.entries.len() >= self.max_entries {
            if let Some(evict) = self.access_order.first().copied() {
                self.entries.remove(&evict);
                self.access_order.remove(0);
            }
        }
        self.entries.insert(key, pipeline);
        self.access_order.push(key);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_equality() {
        let k1 = PipelineKey { color_space: 1, transfer: 2, adjust_enabled: 0 };
        let k2 = PipelineKey { color_space: 1, transfer: 2, adjust_enabled: 0 };
        let k3 = PipelineKey { color_space: 1, transfer: 3, adjust_enabled: 0 };
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn cache_lru_eviction_keys() {
        let mut cache = PipelineCache::new();
        cache.max_entries = 3;

        let keys: Vec<PipelineKey> = (0..4).map(|i| PipelineKey {
            color_space: i, transfer: 0, adjust_enabled: 0,
        }).collect();

        // touch_key simulates an access by moving to back of access_order
        for k in &keys[0..3] {
            cache.access_order.push(*k);
        }
        assert_eq!(cache.access_order.len(), 3);

        // Access key 1 again — move to back (simulating LRU reorder)
        cache.access_order.retain(|k| k != &keys[1]);
        cache.access_order.push(keys[1]);
        assert_eq!(*cache.access_order.last().unwrap(), keys[1]);

        // Insert key 3 at max capacity — oldest (key 0) stays at front
        cache.access_order.push(keys[3]);
        assert_eq!(cache.access_order.len(), 4);
    }
}
