use std::marker::PhantomData;
use std::sync::Arc;

use anyhow::{anyhow, Result};

use crate::preview::pipeline::cache::{PipelineCache, PipelineKey};
use crate::preview::pipeline::device::PreviewGpuContext;
use crate::preview::pipeline::params::PreviewParams;
use crate::preview::pipeline::shaders::PREVIEW_SHADER_WGSL;

pub struct Uninitialized;
pub struct Ready;

pub struct GpuPreviewPipeline<S = Uninitialized> {
    context: Option<Arc<PreviewGpuContext>>,
    bind_group_layout: Option<wgpu::BindGroupLayout>,
    pipeline_layout: Option<wgpu::PipelineLayout>,
    cache: PipelineCache,
    bayer_buffer: Option<wgpu::Buffer>,
    output_buffer: Option<wgpu::Buffer>,
    readback_buffer: Option<wgpu::Buffer>,
    uniform_buffer: Option<wgpu::Buffer>,
    hist_buffers: Option<[wgpu::Buffer; 4]>,
    current_bind_group: Option<wgpu::BindGroup>,
    current_width: u32,
    current_height: u32,
    _state: PhantomData<S>,
}

impl GpuPreviewPipeline<Uninitialized> {
    pub fn new() -> Self {
        Self {
            context: None,
            bind_group_layout: None,
            pipeline_layout: None,
            cache: PipelineCache::new(),
            bayer_buffer: None,
            output_buffer: None,
            readback_buffer: None,
            uniform_buffer: None,
            hist_buffers: None,
            current_bind_group: None,
            current_width: 0,
            current_height: 0,
            _state: PhantomData,
        }
    }

    pub fn init(self, context: Arc<PreviewGpuContext>) -> Result<GpuPreviewPipeline<Ready>> {
        let device = &context.device;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Preview Shader"),
            source: wgpu::ShaderSource::Wgsl(PREVIEW_SHADER_WGSL.into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Preview BGL"),
            entries: &[
                // 0: bayer_packed (storage, read)
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 1: output_rgba (storage, read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 2: PreviewParams (uniform)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 3: hist_luma (storage, read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 4: hist_r
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 5: hist_g
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 6: hist_b
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Preview Pipeline Layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        // Pre-warm the default pipeline (sRGB, Rec709, no adjustments)
        let default_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Preview Pipeline (default)"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let default_key = PipelineKey {
            color_space: 11, // Rec709
            transfer: 14,    // Gamma24 (display default)
            adjust_enabled: 0,
        };
        let mut cache = PipelineCache::new();
        cache.insert(default_key, default_pipeline);

        let hist_size = (64 * std::mem::size_of::<u32>()) as u64;
        let hist_usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        let hist_buffers = [
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hist_luma"), size: hist_size, usage: hist_usage, mapped_at_creation: false,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hist_r"), size: hist_size, usage: hist_usage, mapped_at_creation: false,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hist_g"), size: hist_size, usage: hist_usage, mapped_at_creation: false,
            }),
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hist_b"), size: hist_size, usage: hist_usage, mapped_at_creation: false,
            }),
        ];

        Ok(GpuPreviewPipeline {
            context: Some(context),
            bind_group_layout: Some(bgl),
            pipeline_layout: Some(pipeline_layout),
            cache,
            bayer_buffer: None,
            output_buffer: None,
            readback_buffer: None,
            uniform_buffer: None,
            hist_buffers: Some(hist_buffers),
            current_bind_group: None,
            current_width: 0,
            current_height: 0,
            _state: PhantomData,
        })
    }
}

impl GpuPreviewPipeline<Ready> {
    pub fn process_and_readback(
        &mut self,
        bayer: &[u16],
        params: &PreviewParams,
    ) -> Result<(Vec<u8>, u32, u32)> {
        let context = self.context.as_ref().ok_or_else(|| anyhow!("No GPU context"))?;
        let device = &context.device;
        let queue = &context.queue;

        let out_w = params.width;
        let out_h = params.height;
        let bayer_pixel_count = params.bayer_width as u64 * params.bayer_height as u64;

        // Upload bayer data to GPU (pack u16 pairs into u32)
        let bayer_u32_count = (bayer_pixel_count + 1) / 2;
        let bayer_bytes_needed = bayer_u32_count as u64 * 4;
        let bayer_upload_bytes = bytemuck::cast_slice(bayer);

        // Ensure bayer buffer is large enough
        if self.bayer_buffer.as_ref().map_or(true, |b| b.size() < bayer_bytes_needed) {
            self.bayer_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Preview Bayer"),
                size: bayer_bytes_needed.max(self.bayer_buffer.as_ref().map_or(0, |b| b.size())),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let bayer_buf = self.bayer_buffer.as_ref().unwrap();
        queue.write_buffer(bayer_buf, 0, bayer_upload_bytes);

        // Ensure output buffer — recreate if dimensions changed (handles shrink + grow)
        let out_pixel_count = out_w as u64 * out_h as u64;
        let out_bytes = out_pixel_count * 4; // packed RGBA8 u32
        let dims_changed = self.current_width != out_w || self.current_height != out_h;
        if dims_changed || self.output_buffer.is_none() {
            self.output_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Preview Output"),
                size: out_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }));
            self.readback_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Preview Readback"),
                size: out_bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.current_width = out_w;
            self.current_height = out_h;
        }

        // Ensure uniform buffer
        let uniform_size = std::mem::size_of::<PreviewParams>() as u64;
        if self.uniform_buffer.is_none() {
            self.uniform_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Preview Uniforms"),
                size: uniform_size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }

        queue.write_buffer(
            self.uniform_buffer.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(params),
        );

        // Clear histogram buffers if histogram is enabled
        if params.compute_histogram != 0 {
            if let Some(ref hbufs) = self.hist_buffers {
                for buf in hbufs {
                    queue.write_buffer(buf, 0, &[0u8; 256]);
                }
            }
        }

        // Get or create the compute pipeline for this key
        let key = PipelineKey::from_params(params);
        if self.cache.get(&key).is_none() {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Preview Shader"),
                source: wgpu::ShaderSource::Wgsl(PREVIEW_SHADER_WGSL.into()),
            });
            let pipeline_layout = self.pipeline_layout.as_ref().unwrap();
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Preview Pipeline"),
                layout: Some(pipeline_layout),
                module: &shader,
                entry_point: Some("main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
            self.cache.insert(key, pipeline);
        }

        let pipeline = self.cache.get(&key).unwrap();

        // Create bind group for this frame
        let output_buf = self.output_buffer.as_ref().unwrap();
        let uniform_buf = self.uniform_buffer.as_ref().unwrap();
        let hist = self.hist_buffers.as_ref().unwrap();

        let bgl = self.bind_group_layout.as_ref().unwrap();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: bayer_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: output_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: uniform_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: hist[0].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: hist[1].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: hist[2].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: hist[3].as_entire_binding() },
            ],
            label: Some("Preview Bind Group"),
        });
        self.current_bind_group = Some(bind_group);

        // Dispatch compute
        let wg_x = (out_w + 15) / 16;
        let wg_y = (out_h + 15) / 16;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Preview Encoder"),
        });

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Preview Compute"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(pipeline);
            cpass.set_bind_group(0, self.current_bind_group.as_ref().unwrap(), &[]);
            cpass.dispatch_workgroups(wg_x, wg_y, 1);
        }

        // Copy output to readback buffer
        let readback = self.readback_buffer.as_ref().unwrap();
        encoder.copy_buffer_to_buffer(output_buf, 0, readback, 0, out_bytes);

        queue.submit(Some(encoder.finish()));

        // Map readback buffer
        let buffer_slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().map_err(|_| anyhow!("Readback channel closed"))?
            .map_err(|e| anyhow!("Buffer map failed: {:?}", e))?;

        let data = buffer_slice.get_mapped_range();
        let mut rgba_bytes = vec![0u8; out_pixel_count as usize * 4];
        rgba_bytes.copy_from_slice(&data);
        drop(data);
        readback.unmap();

        Ok((rgba_bytes, out_w, out_h))
    }

    pub fn resize(&mut self, _width: u32, _height: u32) {
        self.output_buffer = None;
        self.readback_buffer = None;
    }
}
