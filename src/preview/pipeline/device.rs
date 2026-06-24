use anyhow::{anyhow, Result};
use std::sync::Arc;

pub struct PreviewGpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl PreviewGpuContext {
    pub fn new() -> Result<Self> {
        let context = pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .ok_or_else(|| anyhow!("No GPU adapter found for preview pipeline"))?;

            let (device, queue) = adapter
                .request_device(
                    &wgpu::DeviceDescriptor {
                        label: Some("mcraw-tui Preview GPU"),
                        required_features: wgpu::Features::empty(),
                        required_limits: wgpu::Limits::default(),
                        memory_hints: wgpu::MemoryHints::Performance,
                    },
                    None,
                )
                .await
                .map_err(|e| anyhow!("Failed to create preview GPU device: {}", e))?;

            Ok::<Self, anyhow::Error>(Self { device, queue })
        })?;
        Ok(context)
    }

    pub fn from_shared(context: &Arc<crate::gpu::GpuContext>) -> Self {
        Self {
            device: context.device.clone(),
            queue: context.queue.clone(),
        }
    }
}
