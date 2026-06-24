use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameKey {
    pub timestamp_ns: i64,
}

#[derive(Debug)]
pub struct CachedFrame {
    pub timestamp_ns: i64,
    pub bayer: Vec<u16>,
    pub last_access: Instant,
}

impl CachedFrame {
    pub fn byte_size(&self) -> usize {
        self.bayer.len() * 2
    }
}

pub struct FrameCache {
    entries: HashMap<FrameKey, CachedFrame>,
    max_bytes: usize,
    current_bytes: usize,
}

impl FrameCache {
    pub fn new(max_bytes_mb: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_bytes: max_bytes_mb * 1024 * 1024,
            current_bytes: 0,
        }
    }

    pub fn get(&mut self, key: &FrameKey) -> Option<&CachedFrame> {
        if let Some(frame) = self.entries.get_mut(key) {
            frame.last_access = Instant::now();
            return Some(frame);
        }
        None
    }

    pub fn insert(&mut self, frame: CachedFrame) {
        let key = FrameKey { timestamp_ns: frame.timestamp_ns };
        let frame_bytes = frame.byte_size();

        if let Some(old) = self.entries.remove(&key) {
            self.current_bytes -= old.byte_size();
        }

        while self.current_bytes + frame_bytes > self.max_bytes && !self.entries.is_empty() {
            if let Some(oldest_key) = self.entries
                .iter()
                .min_by_key(|(_, v)| v.last_access)
                .map(|(k, _)| *k)
            {
                if let Some(old) = self.entries.remove(&oldest_key) {
                    self.current_bytes -= old.byte_size();
                }
            }
        }

        self.current_bytes += frame_bytes;
        self.entries.insert(key, frame);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.current_bytes = 0;
    }

    pub fn current_bytes(&self) -> usize {
        self.current_bytes
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
    fn cache_eviction() {
        let mut cache = FrameCache::new(1); // 1 MB
        let small_frame = CachedFrame {
            timestamp_ns: 0,
            bayer: vec![0u16; 100], // 200 bytes
            last_access: Instant::now(),
        };
        cache.insert(small_frame);
        assert_eq!(cache.len(), 1);

        let key = FrameKey { timestamp_ns: 0 };
        assert!(cache.get(&key).is_some());
    }

    #[test]
    fn cache_clear() {
        let mut cache = FrameCache::new(256);
        let frame = CachedFrame {
            timestamp_ns: 42,
            bayer: vec![0u16; 1000],
            last_access: Instant::now(),
        };
        cache.insert(frame);
        assert!(!cache.is_empty());
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.current_bytes(), 0);
    }
}
