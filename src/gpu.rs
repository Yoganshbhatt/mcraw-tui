use anyhow::{anyhow, Result};
use std::sync::Arc;

pub struct GpuContext { pub device: wgpu::Device, pub queue: wgpu::Queue }
impl GpuContext {
    pub async fn new() -> Result<Self> {
        let instance = wgpu::Instance::default();
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.ok_or_else(|| anyhow!("Failed to find a suitable GPU adapter"))?;
        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor { label: Some("mcraw-tui GPU"), required_features: wgpu::Features::empty(), required_limits: wgpu::Limits::default(), memory_hints: wgpu::MemoryHints::Performance }, None).await.map_err(|e| anyhow!("Failed to create GPU device: {}", e))?;
        Ok(Self { device, queue })
    }
}

pub struct RcdPipeline {
    context: Arc<GpuContext>, width: u32, height: u32, cfa_texture: wgpu::Texture, vh_texture: wgpu::Texture,
    pq_texture: wgpu::Texture, lp_texture: wgpu::Texture, out_buffer: wgpu::Buffer, readback_buffer: wgpu::Buffer,
    conv_pipeline: wgpu::ComputePipeline, conv_bind_group: wgpu::BindGroup, fill_pipeline: wgpu::ComputePipeline,
    fill_bind_group: wgpu::BindGroup, uniform_buffer: wgpu::Buffer, sampler: wgpu::Sampler,
    conv_bgl: wgpu::BindGroupLayout, fill_bgl: wgpu::BindGroupLayout,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuUniforms {
    width: u32, height: u32, filters: u32, gamma_mode: u32,
    black_level: f32, white_level: f32, wb_r: f32, wb_b: f32,
    black_r: f32, black_g: f32, black_b: f32, _black_pad: f32,
    ccm_row0: [f32; 4], ccm_row1: [f32; 4], ccm_row2: [f32; 4],
    phase_x: i32, phase_y: i32, _pad: [u32; 2],
}

fn transfer_to_gamma_mode(tf: &crate::color::TransferFunction) -> u32 {
    use crate::color::TransferFunction;
    match tf {
        TransferFunction::Linear => 0, TransferFunction::Rec709 => 1,
        TransferFunction::SLog3 => 2, TransferFunction::VLog => 3, TransferFunction::ARRIlog3 => 4,
        TransferFunction::CLog3 => 5, TransferFunction::FLog2 => 6, TransferFunction::ACESCCT => 7,
        TransferFunction::PQ => 8, TransferFunction::HLG => 9, TransferFunction::DaVinciIntermediate => 10,
        TransferFunction::AppleLog | TransferFunction::AppleLog2 => 11,
        TransferFunction::Gamma24 => 12,
    }
}

impl RcdPipeline {
    pub fn new(context: Arc<GpuContext>, width: u32, height: u32) -> Result<Self> {
        let device = &context.device;
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, address_mode_w: wgpu::AddressMode::ClampToEdge, mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest, mipmap_filter: wgpu::FilterMode::Nearest, ..Default::default() });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("RCD Uniforms"), size: std::mem::size_of::<GpuUniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let conv_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("RCD Conv"), source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/rcd_conv.wgsl").into()) });
        let fill_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("RCD Fill"), source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/rcd_fill.wgsl").into()) });
        
        let conv_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("RCD Conv BGL"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: wgpu::TextureFormat::R32Float, view_dimension: wgpu::TextureViewDimension::D2 }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: wgpu::TextureFormat::R32Float, view_dimension: wgpu::TextureViewDimension::D2 }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: wgpu::TextureFormat::R32Float, view_dimension: wgpu::TextureViewDimension::D2 }, count: None },
        ]});
        let conv_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("RCD Conv Layout"), bind_group_layouts: &[&conv_bgl], push_constant_ranges: &[] });
        let conv_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("RCD Conv"), layout: Some(&conv_pipeline_layout), module: &conv_shader, entry_point: Some("main"), compilation_options: wgpu::PipelineCompilationOptions::default(), cache: None });
        
        let fill_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("RCD Fill BGL"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ]});
        let fill_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("RCD Fill Layout"), bind_group_layouts: &[&fill_bgl], push_constant_ranges: &[] });
        let fill_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("RCD Fill"), layout: Some(&fill_pipeline_layout), module: &fill_shader, entry_point: Some("main"), compilation_options: wgpu::PipelineCompilationOptions::default(), cache: None });
        
        let out_size = 8u64; // Dummy size
        let out_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("RCD Out Buffer"), size: out_size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("RCD Readback Buffer"), size: out_size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        
        let dummy_u16 = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R16Uint, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
        let dummy_f32 = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R32Float, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING, view_formats: &[] });
        let dummy_storage = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R32Float, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING, view_formats: &[] });
        
        let conv_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { layout: &conv_bgl, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&dummy_u16.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: uniform_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&dummy_storage.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&dummy_storage.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&dummy_storage.create_view(&Default::default())) },
        ], label: None });
        
        let fill_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { layout: &fill_bgl, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&dummy_u16.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&dummy_f32.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&dummy_f32.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&dummy_f32.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 4, resource: out_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: uniform_buffer.as_entire_binding() },
        ], label: None });
        
        let mut pipeline = Self {
            context: context.clone(), width: 0, height: 0, cfa_texture: dummy_u16, vh_texture: dummy_f32, pq_texture: dummy_storage,
            lp_texture: device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R32Float, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING, view_formats: &[] }),
            out_buffer, readback_buffer, conv_pipeline, conv_bind_group, fill_pipeline, fill_bind_group, conv_bgl, fill_bgl, uniform_buffer, sampler,
        };
        pipeline.resize(width, height)?;
        Ok(pipeline)
    }

    pub fn process(&mut self, bayer: &[u16], filters: u32, black_level: f32, white_level: f32, stride_width: u32, offset_x: u32, offset_y: u32, fused_ccm: &[f32; 9], as_shot_neutral: &[f32; 3], tf: &crate::color::TransferFunction) -> Result<Vec<u32>> {
        let device = &self.context.device; let queue = &self.context.queue;
        let mut ccm_row0 = [0.0f32; 4]; let mut ccm_row1 = [0.0f32; 4]; let mut ccm_row2 = [0.0f32; 4];
        ccm_row0[..3].copy_from_slice(&fused_ccm[0..3]); ccm_row1[..3].copy_from_slice(&fused_ccm[3..6]); ccm_row2[..3].copy_from_slice(&fused_ccm[6..9]);
        let raw_wb_r = if as_shot_neutral[0] > 1e-6 { as_shot_neutral[1] / as_shot_neutral[0] } else { 1.0 };
        let raw_wb_b = if as_shot_neutral[2] > 1e-6 { as_shot_neutral[1] / as_shot_neutral[2] } else { 1.0 };
        let wb_r = raw_wb_r.clamp(0.1, 10.0);
        let wb_b = raw_wb_b.clamp(0.1, 10.0);
        if (wb_r - raw_wb_r).abs() > 1e-3 || (wb_b - raw_wb_b).abs() > 1e-3 {
            tracing::warn!(
                "WB gains clamped: as_shot_neutral={:?} raw=[{:.3},{:.3}] clamped=[{:.3},{:.3}]",
                as_shot_neutral, raw_wb_r, raw_wb_b, wb_r, wb_b
            );
        }
        // Per-channel black level. When the decoder supplies only one
        // value, all four channels use that single black level.
        let bl = black_level;
        let uniforms = GpuUniforms {
            width: self.width, height: self.height, filters, gamma_mode: transfer_to_gamma_mode(tf),
            black_level: bl, white_level,
            wb_r, wb_b,
            black_r: bl, black_g: bl, black_b: bl, _black_pad: 0.0,
            ccm_row0, ccm_row1, ccm_row2,
            phase_x: (offset_x & 1) as i32, phase_y: (offset_y & 1) as i32, _pad: [0u32; 2],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        
        const ALIGN: u32 = 256; let bayer_bytes = bytemuck::cast_slice(bayer); let src_stride = (stride_width * 2) as u32;
        let aligned_stride = ((src_stride + ALIGN - 1) / ALIGN) * ALIGN; let row_bytes = self.width as usize * 2;
        let mut upload_data = vec![0u8; aligned_stride as usize * self.height as usize];
        for row in 0..self.height as usize {
            let src_off = ((offset_y as usize + row) * stride_width as usize + offset_x as usize) * 2;
            let dst_off = row * aligned_stride as usize;
            upload_data[dst_off..dst_off + row_bytes].copy_from_slice(&bayer_bytes[src_off..src_off + row_bytes]);
        }
        queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &self.cfa_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All }, &upload_data, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(aligned_stride), rows_per_image: Some(self.height) }, wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 });
        
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("RCD Encoder") });
        let wg_conv_x = (self.width + 15) / 16; let wg_conv_y = (self.height + 15) / 16;
        let valid_x = 128u32.saturating_sub(18); let valid_y = 32u32.saturating_sub(18);
        let wg_fill_x = (self.width + valid_x - 1) / valid_x; let wg_fill_y = (self.height + valid_y - 1) / valid_y;
        
        let conv_bg = device.create_bind_group(&wgpu::BindGroupDescriptor { layout: &self.conv_bgl, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.cfa_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: self.uniform_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.vh_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&self.pq_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&self.lp_texture.create_view(&Default::default())) },
        ], label: Some("RCD Conv BG") });
        
        let fill_bg = device.create_bind_group(&wgpu::BindGroupDescriptor { layout: &self.fill_bgl, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.cfa_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.vh_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.pq_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.lp_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 4, resource: self.out_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: self.uniform_buffer.as_entire_binding() },
        ], label: Some("RCD Fill BG") });
        
        { let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("RCD Conv"), timestamp_writes: None }); cpass.set_pipeline(&self.conv_pipeline); cpass.set_bind_group(0, &conv_bg, &[]); cpass.dispatch_workgroups(wg_conv_x, wg_conv_y, 1); }
        { let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("RCD Fill"), timestamp_writes: None }); cpass.set_pipeline(&self.fill_pipeline); cpass.set_bind_group(0, &fill_bg, &[]); cpass.dispatch_workgroups(wg_fill_x.max(1), wg_fill_y.max(1), 1); }
        
        // FIXED: 16-bit output size (8 bytes per pixel)
        let out_size = (self.width as u64) * (self.height as u64) * 8;
        encoder.copy_buffer_to_buffer(&self.out_buffer, 0, &self.readback_buffer, 0, out_size);
        queue.submit(Some(encoder.finish()));
        
        let buffer_slice = self.readback_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| { let _ = tx.send(result); });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().map_err(|_| anyhow!("Readback recv failed"))?.map_err(|e| anyhow!("Buffer map failed: {:?}", e))?;
        let data = buffer_slice.get_mapped_range();
        let u32_data: &[u32] = bytemuck::cast_slice(&data);
        let pixel_count = (self.width * self.height) as usize;
        
        // FIXED: Read 2 u32s per pixel
        let result = u32_data[..pixel_count * 2].to_vec();
        drop(data); self.readback_buffer.unmap();
        Ok(result)
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        let device = &self.context.device;
        let tex_desc = |format: wgpu::TextureFormat, usage: wgpu::TextureUsages| wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] };
        self.cfa_texture = device.create_texture(&tex_desc(wgpu::TextureFormat::R16Uint, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST));
        self.vh_texture = device.create_texture(&tex_desc(wgpu::TextureFormat::R32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING));
        // Both P/Q and LPF pyramid are written at half-res in both X
        // and Y by the conv shader (LuisSR steps 1 and 4). Allocating
        // half-height too avoids wasting the upper half of the texture.
        let half_w = (width + 1) / 2;
        let half_h = (height + 1) / 2;
        let half_desc = |format: wgpu::TextureFormat, usage: wgpu::TextureUsages| wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: half_w, height: half_h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] };
        self.pq_texture = device.create_texture(&half_desc(wgpu::TextureFormat::R32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING));
        self.lp_texture = device.create_texture(&half_desc(wgpu::TextureFormat::R32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING));
        
        // FIXED: 16-bit output size (8 bytes per pixel)
        let out_size = (width as u64) * (height as u64) * 8;
        self.out_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("RCD Out Buffer"), size: out_size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        self.readback_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("RCD Readback Buffer"), size: out_size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        self.width = width; self.height = height;
        
        self.conv_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { layout: &self.conv_pipeline.get_bind_group_layout(0), entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.cfa_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: self.uniform_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.vh_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&self.pq_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&self.lp_texture.create_view(&Default::default())) },
        ], label: None });
        
        self.fill_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { layout: &self.fill_pipeline.get_bind_group_layout(0), entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.cfa_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.vh_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.pq_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.lp_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 4, resource: self.out_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: self.uniform_buffer.as_entire_binding() },
        ], label: None });
        Ok(())
    }
}