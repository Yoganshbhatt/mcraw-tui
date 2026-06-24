pub mod encoders;
pub mod pipeline;
pub mod state;

use std::time::Instant;

#[derive(Debug)]
pub enum PreviewState {
    Empty,
    Loading { started: Instant },
    Ready {
        sixel: Vec<u8>,
        width: u32,
        height: u32,
    },
    Error(String),
}

impl Default for PreviewState {
    fn default() -> Self {
        Self::Empty
    }
}

impl PreviewState {
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}
