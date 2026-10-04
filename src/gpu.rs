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
    context: Arc<GpuContext>, width: u32, height: u32, aligned_stride: u32, upload_data: Vec<u8>,
    cfa_texture: wgpu::Texture, vh_texture: wgpu::Texture,
    pq_texture: wgpu::Texture, lp_texture: wgpu::Texture, out_buffer: wgpu::Buffer, readback_buffer: wgpu::Buffer,
    conv_pipeline: wgpu::ComputePipeline, conv_bind_group: wgpu::BindGroup, fill_pipeline: wgpu::ComputePipeline, fill_basic_pipeline: wgpu::ComputePipeline,
    fill_bind_group: wgpu::BindGroup, uniform_buffer: wgpu::Buffer, sampler: wgpu::Sampler,
    conv_bgl: wgpu::BindGroupLayout, fill_bgl: wgpu::BindGroupLayout,
    hl_pipeline: wgpu::ComputePipeline, hl_bind_group: wgpu::BindGroup,
    hl_uniform_buffer: wgpu::Buffer, hl_out_buffer: wgpu::Buffer, cfa_fixed: wgpu::Texture,
}

/// Uniforms for the `hl_complete.wgsl` pass. Pure vec4 members only, so the
/// WGSL 16-byte alignment and Rust 4-byte array alignment agree by
/// construction (the D7 lesson). 5 x 16 = 80 bytes. `GpuUniforms` is frozen
/// and is NOT extended for this pass.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HlUniforms {
    dims: [u32; 4],
    range: [f32; 4],
    black: [f32; 4],
    neutral: [f32; 4],
    aux: [u32; 4],
}

/// Folds a CFA pattern to its 0..3 base for the shader, exactly matching
/// `hl::color_at` (whose Quad arms are identical to the base arms).
fn hl_base_pattern(pattern: crate::file::BayerPattern) -> u32 {
    use crate::file::BayerPattern;
    match pattern {
        BayerPattern::RGGB | BayerPattern::QuadBayerRGGB => 0,
        BayerPattern::GRBG | BayerPattern::QuadBayerGRBG => 1,
        BayerPattern::GBRG | BayerPattern::QuadBayerGBRG => 2,
        BayerPattern::BGGR | BayerPattern::QuadBayerBGGR => 3,
    }
}

/// Converts one [`crate::hl::HlParams`] to the uniform block. One conversion,/// one source of constants: `pipeline.rs` passes the same params the CPU
/// path would use. `enabled` selects Full completion vs the Sensor identity
/// copy (the shader never runs the Prior middle arm).
fn hl_params_to_uniform(p: &crate::hl::HlParams, width: u32, height: u32, pattern: crate::file::BayerPattern, pitch_words: u32, enabled: bool) -> HlUniforms {
    HlUniforms {
        dims: [width, height, hl_base_pattern(pattern), pitch_words],
        range: [p.range[0], p.range[1], p.range[2], p.noise_floor],
        black: [p.black[0], p.black[1], p.black[2], p.rail_dn],
        neutral: [p.neutral[0], p.neutral[1], p.neutral[2], p.fallback_scale],
        aux: [u32::from(enabled), 0, 0, 0],
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuUniforms {
    width: u32, height: u32, filters: u32, gamma_mode: u32,
    black_level: f32, white_level: f32, wb_r: f32, wb_b: f32,
    black_r: f32, black_g: f32, black_b: f32, _black_pad: f32,
    ccm_row0: [f32; 4], ccm_row1: [f32; 4], ccm_row2: [f32; 4],
    phase_x: i32, phase_y: i32,
    // Trailing pad so the struct is a multiple of 16 bytes (112). WGSL
    // uniform blocks require that; at 104 every GPU-path dispatch faults.
    // Trailing only — no existing member moves.
    _pad: [u32; 2],
}

fn transfer_to_gamma_mode(tf: &crate::color::TransferFunction) -> u32 {
    use crate::color::TransferFunction;
    match tf {
        TransferFunction::Linear => 0, TransferFunction::Rec709 => 1,
        TransferFunction::SLog3 => 2, TransferFunction::VLog => 3, TransferFunction::ARRIlog3 => 4,
        TransferFunction::ARRIlog4 => 13, TransferFunction::CLog3 => 5, TransferFunction::FLog2 => 6,
        TransferFunction::ACESCCT => 7, TransferFunction::PQ => 8, TransferFunction::HLG => 9,
        TransferFunction::DaVinciIntermediate => 10,
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
        // OFF/basic entry point sharing the same module and layout: the
        // classic neutral desaturation instead of the reconstruction path.
        // Same bind-group layout, so both pipelines share `fill_bind_group`.
        let fill_basic_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("RCD Fill Basic"), layout: Some(&fill_pipeline_layout), module: &fill_shader, entry_point: Some("main_basic"), compilation_options: wgpu::PipelineCompilationOptions::default(), cache: None });

        let hl_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("HL Complete"), source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/hl_complete.wgsl").into()) });
        let hl_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("HL Uniforms"), size: std::mem::size_of::<HlUniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let hl_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("HL Complete BGL"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ]});
        let hl_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("HL Complete Layout"), bind_group_layouts: &[&hl_bgl], push_constant_ranges: &[] });
        let hl_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("HL Complete"), layout: Some(&hl_pipeline_layout), module: &hl_shader, entry_point: Some("main"), compilation_options: wgpu::PipelineCompilationOptions::default(), cache: None });
        
        let out_size = 8u64; // Dummy size
        let out_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("RCD Out Buffer"), size: out_size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("RCD Readback Buffer"), size: out_size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let hl_out_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("HL Out Buffer"), size: out_size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        
        let dummy_u16 = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R16Uint, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
        let dummy_f32 = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R32Float, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING, view_formats: &[] });
        let cfa_fixed = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R16Uint, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
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

        let hl_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { layout: &hl_bgl, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&dummy_u16.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: hl_out_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: hl_uniform_buffer.as_entire_binding() },
        ], label: None });
        
        let mut pipeline = Self {
            context: context.clone(), width: 0, height: 0, aligned_stride: 0, upload_data: Vec::new(), cfa_texture: dummy_u16, vh_texture: dummy_f32, pq_texture: dummy_storage,
            lp_texture: device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::R32Float, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING, view_formats: &[] }),
            out_buffer, readback_buffer, conv_pipeline, conv_bind_group, fill_pipeline, fill_basic_pipeline, fill_bind_group, conv_bgl, fill_bgl, uniform_buffer, sampler,
            hl_pipeline, hl_bind_group, hl_uniform_buffer, hl_out_buffer, cfa_fixed,
        };
        pipeline.resize(width, height)?;
        Ok(pipeline)
    }

    /// Runs the full GPU chain: highlight completion -> RCD conv -> RCD fill.
    /// `hl_params` carries the export's params (also selects the fill entry:
    /// `Full` runs the pure reconstruction path, anything else the basic
    /// desaturation). `run_hl` gates the on-device completion pass: true
    /// ONLY on a raw-domain mosaic with matching params (lens correction off,
    /// or the CPU pass provably skipped). False runs the identity copy —
    /// Sensor exports, or the CPU already completed pre-lens. The caller owns
    /// mutual exclusion: this function cannot tell a completed mosaic from
    /// a raw one, so `run_hl=true` on completed data double-completes in the
    /// wrong domain (sun core 0.47x level, found 2026-09-27). Conv/fill
    /// always read `cfa_fixed` through a single code path. The repaired
    /// mosaic stays resident on the GPU: no readback is added by this pass.
    pub fn process(&mut self, bayer: &[u16], filters: u32, black_r: f32, black_g: f32, black_b: f32, white_level: f32, stride_width: u32, offset_x: u32, offset_y: u32, fused_ccm: &[f32; 9], as_shot_neutral: &[f32; 3], tf: &crate::color::TransferFunction, hl_params: &crate::hl::HlParams, run_hl: bool, pattern: crate::file::BayerPattern) -> Result<Vec<u8>> {
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
        // Per-channel black levels, resolved host-side (per-frame dynamic /
        // static per-channel values, or the single header value broadcast by
        // the caller when the file carries no per-channel data). The scalar
        // `black_level` uniform field has no WGSL consumer since per-channel
        // normalize landed — kept for struct-layout stability (pinned test).
        let uniforms = GpuUniforms {
            width: self.width, height: self.height, filters, gamma_mode: transfer_to_gamma_mode(tf),
            black_level: black_r, white_level,
            wb_r, wb_b,
            black_r, black_g, black_b, _black_pad: 0.0,
            ccm_row0, ccm_row1, ccm_row2,
            phase_x: (offset_x & 1) as i32, phase_y: (offset_y & 1) as i32,
            _pad: [0, 0],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        
        let bayer_bytes = bytemuck::cast_slice(bayer); let row_bytes = self.width as usize * 2;
        self.upload_data.fill(0);
        for row in 0..self.height as usize {
            let src_off = ((offset_y as usize + row) * stride_width as usize + offset_x as usize) * 2;
            let dst_off = row * self.aligned_stride as usize;
            self.upload_data[dst_off..dst_off + row_bytes].copy_from_slice(&bayer_bytes[src_off..src_off + row_bytes]);
        }
        queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &self.cfa_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All }, &self.upload_data, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.aligned_stride), rows_per_image: Some(self.height) }, wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 });

        let hl_enabled = run_hl;
        // pitch_words is u32 words per row: aligned_stride is BYTES.
        let hl_uniforms = hl_params_to_uniform(hl_params, self.width, self.height, pattern, self.aligned_stride / 4, hl_enabled);
        queue.write_buffer(&self.hl_uniform_buffer, 0, bytemuck::bytes_of(&hl_uniforms));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("RCD Encoder") });
        // Pair-grid: one thread owns two horizontal photosites, so the x
        // dispatch covers ceil(width/2) threads.
        let wg_hl_x = ((self.width + 1) / 2 + 15) / 16; let wg_hl_y = (self.height + 15) / 16;
        { let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("HL Complete"), timestamp_writes: None }); cpass.set_pipeline(&self.hl_pipeline); cpass.set_bind_group(0, &self.hl_bind_group, &[]); cpass.dispatch_workgroups(wg_hl_x.max(1), wg_hl_y.max(1), 1); }
        // Device-side only: packed completion buffer -> fixed CFA texture.
        // bytes_per_row reuses aligned_stride, which resize() guarantees is
        // a multiple of 256 as copy_buffer_to_texture requires.
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo { buffer: &self.hl_out_buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.aligned_stride), rows_per_image: Some(self.height) } },
            wgpu::TexelCopyTextureInfo { texture: &self.cfa_fixed, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 });
        let wg_conv_x = (self.width + 15) / 16; let wg_conv_y = (self.height + 15) / 16;
        let valid_x = 128u32.saturating_sub(18); let valid_y = 32u32.saturating_sub(18);
        let wg_fill_x = (self.width + valid_x - 1) / valid_x; let wg_fill_y = (self.height + valid_y - 1) / valid_y;
        
        { let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("RCD Conv"), timestamp_writes: None }); cpass.set_pipeline(&self.conv_pipeline); cpass.set_bind_group(0, &self.conv_bind_group, &[]); cpass.dispatch_workgroups(wg_conv_x, wg_conv_y, 1); }
        // Fill entry follows the recovery POLICY, not HL execution: a
        // lens-on ON export has CPU-completed data with an identity device
        // pass, but still needs the pure reconstruction fill (no desat).
        // Same bind group (identical layouts).
        let fill_pipe = if hl_params.policy == crate::hl::HlPolicy::Full { &self.fill_pipeline } else { &self.fill_basic_pipeline };
        { let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("RCD Fill"), timestamp_writes: None }); cpass.set_pipeline(fill_pipe); cpass.set_bind_group(0, &self.fill_bind_group, &[]); cpass.dispatch_workgroups(wg_fill_x.max(1), wg_fill_y.max(1), 1); }

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

        // A6: pack to RGB48LE bytes in a tight loop while the mapped range
        // is live. Per the WGSL: first u32 = (R | G<<16), second u32 = (B | pad<<16).
        let mut frame_bytes = vec![0u8; pixel_count * 6];
        for pi in 0..pixel_count {
            let p0 = u32_data[pi * 2];
            let p1 = u32_data[pi * 2 + 1];
            let r = (p0 & 0xFFFF) as u16;
            let g = ((p0 >> 16) & 0xFFFF) as u16;
            let b = (p1 & 0xFFFF) as u16;
            let o = pi * 6;
            frame_bytes[o]     = r as u8;
            frame_bytes[o + 1] = (r >> 8) as u8;
            frame_bytes[o + 2] = g as u8;
            frame_bytes[o + 3] = (g >> 8) as u8;
            frame_bytes[o + 4] = b as u8;
            frame_bytes[o + 5] = (b >> 8) as u8;
        }
        drop(data); self.readback_buffer.unmap();
        Ok(frame_bytes)
    }

    /// Test-only: runs the HL pass over a tight active-region mosaic and
    /// reads the packed completion buffer back to u16 values.
    ///
    /// NOT in the export flow (which adds no readback): exists so the
    /// CPU<->GPU parity test can assert on values. `active` is row-tight
    /// (`stride == width`, offset 0) and must match the pipeline geometry.
    #[cfg(test)]
    pub fn hl_complete_for_test(&mut self, active: &[u16], params: &crate::hl::HlParams, pattern: crate::file::BayerPattern) -> Result<Vec<u16>> {
        let w = self.width as usize;
        let h = self.height as usize;
        assert_eq!(active.len(), w * h, "test mosaic must match the pipeline geometry");
        let device = &self.context.device;
        let queue = &self.context.queue;
        let bytes: &[u8] = bytemuck::cast_slice(active);
        self.upload_data.fill(0);
        for row in 0..h {
            let dst = row * self.aligned_stride as usize;
            self.upload_data[dst..dst + w * 2].copy_from_slice(&bytes[row * w * 2..(row + 1) * w * 2]);
        }
        queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &self.cfa_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All }, &self.upload_data, wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.aligned_stride), rows_per_image: Some(self.height) }, wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 });
        let enabled = params.policy == crate::hl::HlPolicy::Full;
        // pitch_words is u32 words per row: aligned_stride is BYTES.
        let uniforms = hl_params_to_uniform(params, self.width, self.height, pattern, self.aligned_stride / 4, enabled);
        queue.write_buffer(&self.hl_uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        let pitch_words = self.aligned_stride / 4;
        let word_count = pitch_words as u64 * self.height as u64;
        let staging = device.create_buffer(&wgpu::BufferDescriptor { label: Some("HL Test Readback"), size: word_count * 4, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("HL Test") });
        { let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("HL Complete Test"), timestamp_writes: None }); cpass.set_pipeline(&self.hl_pipeline); cpass.set_bind_group(0, &self.hl_bind_group, &[]); cpass.dispatch_workgroups((((w as u32 + 1) / 2 + 15) / 16).max(1), ((self.height + 15) / 16).max(1), 1); }
        encoder.copy_buffer_to_buffer(&self.hl_out_buffer, 0, &staging, 0, word_count * 4);
        queue.submit(Some(encoder.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().map_err(|_| anyhow!("HL test readback recv failed"))?.map_err(|e| anyhow!("HL test map failed: {e:?}"))?;
        let data = slice.get_mapped_range();
        let words: &[u32] = bytemuck::cast_slice(&data);
        let mut out = vec![0u16; w * h];
        for y in 0..h {
            for x in 0..w {
                let word = words[y * pitch_words as usize + x / 2];
                out[y * w + x] = if x % 2 == 0 { (word & 0xFFFF) as u16 } else { (word >> 16) as u16 };
            }
        }
        drop(data);
        staging.unmap();
        Ok(out)
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {        let device = &self.context.device;
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
        // A3: Hoist upload_data Vec<u8> + aligned_stride to fields, allocated once per resize.
        const ALIGN: u32 = 256;
        let src_stride = width * 2;
        self.aligned_stride = ((src_stride + ALIGN - 1) / ALIGN) * ALIGN;
        debug_assert!(self.aligned_stride % 256 == 0, "copy_buffer_to_texture requires bytes_per_row % 256 == 0");
        // Packed HL output: one u32 word per photosite pair, same pitch.
        let hl_size = self.aligned_stride as u64 * height as u64;
        self.hl_out_buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("HL Out Buffer"), size: hl_size.max(8), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        // The repaired mosaic: written device-side, read by conv/fill.
        self.cfa_fixed = device.create_texture(&tex_desc(wgpu::TextureFormat::R16Uint, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST));
        self.upload_data = vec![0u8; self.aligned_stride as usize * height as usize];
        self.width = width; self.height = height;
        
        self.conv_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { layout: &self.conv_pipeline.get_bind_group_layout(0), entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.cfa_fixed.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: self.uniform_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.vh_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&self.pq_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&self.lp_texture.create_view(&Default::default())) },
        ], label: None });
        
        self.fill_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { layout: &self.fill_pipeline.get_bind_group_layout(0), entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.cfa_fixed.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.vh_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.pq_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.lp_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 4, resource: self.out_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: self.uniform_buffer.as_entire_binding() },
        ], label: None });
        self.hl_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { layout: &self.hl_pipeline.get_bind_group_layout(0), entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.cfa_texture.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 1, resource: self.hl_out_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: self.hl_uniform_buffer.as_entire_binding() },
        ], label: None });
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::{GpuUniforms, HlUniforms};
    use super::hl_base_pattern;
    use crate::file::BayerPattern;
    use std::mem::{offset_of, size_of};

    /// The WGSL Uniforms struct layout contract (rcd_fill.wgsl). WGSL vec4
    /// alignment is 16 bytes; Rust `[f32; 4]` packs at 4. A divergence here
    /// is a SILENT bind-group failure — no compile error, no wgpu
    /// validation error, wrong pixels. Pin it.
    #[test]
    fn uniform_layout_matches_wgsl() {
        assert_eq!(size_of::<GpuUniforms>(), 112, "struct size must be 112 bytes (WGSL uniform blocks are multiples of 16)");
        assert_eq!(offset_of!(GpuUniforms, phase_x), 96);
        assert_eq!(offset_of!(GpuUniforms, phase_y), 100);
        assert_eq!(offset_of!(GpuUniforms, _pad), 104);
    }

    /// The `hl_complete.wgsl` HlUniforms contract. Pure vec4 members, so the
    /// layout agrees by construction — pinned anyway (D7).
    #[test]
    fn hl_uniform_layout_matches_wgsl() {
        assert_eq!(size_of::<HlUniforms>(), 80);
        assert_eq!(offset_of!(HlUniforms, dims), 0);
        assert_eq!(offset_of!(HlUniforms, range), 16);
        assert_eq!(offset_of!(HlUniforms, black), 32);
        assert_eq!(offset_of!(HlUniforms, neutral), 48);
        assert_eq!(offset_of!(HlUniforms, aux), 64);
    }

    /// Quad patterns fold onto their base exactly as `hl::color_at` does.
    #[test]
    fn hl_base_pattern_folds_quads() {
        assert_eq!(hl_base_pattern(BayerPattern::RGGB), 0);
        assert_eq!(hl_base_pattern(BayerPattern::GRBG), 1);
        assert_eq!(hl_base_pattern(BayerPattern::GBRG), 2);
        assert_eq!(hl_base_pattern(BayerPattern::BGGR), 3);
        assert_eq!(hl_base_pattern(BayerPattern::QuadBayerRGGB), 0);
        assert_eq!(hl_base_pattern(BayerPattern::QuadBayerGRBG), 1);
        assert_eq!(hl_base_pattern(BayerPattern::QuadBayerGBRG), 2);
        assert_eq!(hl_base_pattern(BayerPattern::QuadBayerBGGR), 3);
    }

    /// Pair-grid dispatch arithmetic: `wg_hl_x * 32` threads-of-pairs must
    /// cover every photosite without over-dispatching a full extra tile row.
    #[test]
    fn hl_dispatch_covers_frame() {
        for w in [1u32, 2, 3, 1919, 1920, 4080, 4091, 4096] {
            let pairs = (w + 1) / 2;
            let wg = (pairs + 15) / 16;
            assert!(wg * 16 >= pairs, "width {w}: under-dispatched");
            assert!(wg == 1 || (wg - 1) * 16 < pairs, "width {w}: over-dispatched");
        }
        for h in [1u32, 31, 32, 3072] {
            let wg = (h + 15) / 16;
            assert!(wg * 16 >= h && (wg == 1 || (wg - 1) * 16 < h), "height {h}");
        }
    }

    /// `hl_complete.wgsl` must stay barrier-free and workgroup-memory-free
    /// (naga does not enforce barrier uniformity; the rcd_fill 49 KB pattern
    /// must not be repeated here).
    #[test]
    fn hl_shader_has_no_barriers_or_workgroup_memory() {
        let src = include_str!("../shaders/hl_complete.wgsl");
        // Strip line comments: the header documents the absence, which would
        // otherwise trip the substring check.
        let code: String = src.lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>().join("\n");
        assert!(!code.contains("workgroupBarrier"), "barrier crept into hl_complete.wgsl");
        assert!(!code.contains("var<workgroup>"), "workgroup memory crept into hl_complete.wgsl");
    }

    /// All three WGSL shaders must parse and validate under naga with an
    /// EMPTY capability set — the same posture as the runtime device
    /// (`required_features: Features::empty()`), so reaching for subgroups,
    /// cbrt or f32 atomics fails loudly here instead of on hardware.
    #[test]
    fn shaders_pass_naga_validation() {
        for (name, src) in [
            ("hl_complete", include_str!("../shaders/hl_complete.wgsl")),
            ("rcd_conv", include_str!("../shaders/rcd_conv.wgsl")),
            ("rcd_fill", include_str!("../shaders/rcd_fill.wgsl")),
        ] {
            let module = naga::front::wgsl::parse_str(src)
                .unwrap_or_else(|e| panic!("{name}: WGSL parse error: {e:?}"));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: naga validation error: {e:?}"));
        }
    }

    const PARITY_NEUTRAL: [f32; 3] = [0.5293, 1.0, 0.5879];

    /// Dim neutral background + optional fully censored core, in the given
    /// pattern. Small enough to run on any adapter.
    fn parity_mosaic(w: usize, h: usize, pattern: BayerPattern, censored_core: bool) -> Vec<u16> {
        let mut m = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let c = crate::hl::color_at(x as i32, y as i32, pattern);
                m.push((PARITY_NEUTRAL[c] * 0.3 * 959.0 + 64.0) as u16);
            }
        }
        if censored_core {
            let (cx, cy) = (w / 2, h / 2);
            let r = w.min(h) / 6;
            for y in 0..h {
                for x in 0..w {
                    let d = ((x as i32 - cx as i32).abs().max((y as i32 - cy as i32).abs())) as usize;
                    if d <= r {
                        m[y * w + x] = 1023;
                    }
                }
            }
        }
        m
    }

    /// REGRESSION (was the Rec.709 plateau: the RCD fill crushed
    /// highlight gradients into a flat white slab). A realistic bright ramp
    /// (sub-rail rising through moderately above rail, the shape real
    /// completion output takes after WB/CCM) through the full GPU ON path
    /// must match the clean CPU chain (bilinear + normalize + rolloff +
    /// Rec.709 OETF + pack) to 3 LSB, with strictly increasing band means.
    /// The old level-triggered desat flattened the 0.95+ region (slope ~0);
    /// the fixed path preserves it. Determinism asserted by double-run.
    /// NOTE: inputs stay below the HL rail (1023) so Full completion passes
    /// them through untouched; normalize uses wl=800 so the ramp still
    /// exercises the above-rail (>1.0) display path. A ramp HDR-hot enough
    /// to pack everything at 65535 would plateau on the CPU too (pack
    /// saturation is correct super-white behavior, not a bug).
    #[test]
    fn rcd_above_rail_gradient_not_clamped() {
        use crate::color::{apply_display_rolloff, normalize_linear_per_channel, BilinearDemosaic};
        use crate::file::BayerPattern;
        use crate::hl::{HlParams, HlPolicy};
        let ctx = match pollster::block_on(super::GpuContext::new()) {
            Ok(c) => std::sync::Arc::new(c),
            Err(_) => { eprintln!("SKIP: no GPU adapter"); return; }
        };
        let (w, h) = (128u32, 96u32);
        let pattern = BayerPattern::GBRG;
        let filters = 0x49494949u32;
        // Full policy (ON entry `main`) with the HL rail ABOVE the ramp
        // peak, so completion passes the input through untouched.
        let full = HlParams::new(HlPolicy::Full, 1023.0, [64.0, 64.0, 64.0], [1.0, 1.0, 1.0]);
        let ccm = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        // Neutral ramp 300->1000 DN; normalize wl=800 maps it to 0.32->1.27.
        let mut m = vec![0u16; (w * h) as usize];
        for y in 0..h as usize { for x in 0..w as usize {
            m[y * w as usize + x] = (300 + 700 * x as u32 / (w - 1)) as u16;
        }}
        let gpu_bytes = {
            let mut pipe = super::RcdPipeline::new(ctx.clone(), w, h).expect("RcdPipeline::new");
            pipe.process(&m, filters, 64.0, 64.0, 64.0, 800.0, w, 0, 0,
                         &ccm, &[1.0, 1.0, 1.0], &crate::color::TransferFunction::Rec709,
                         &full, true, pattern).expect("RCD process")
        };
        let gpu_bytes2 = {
            let mut pipe = super::RcdPipeline::new(ctx, w, h).expect("RcdPipeline::new");
            pipe.process(&m, filters, 64.0, 64.0, 64.0, 800.0, w, 0, 0,
                         &ccm, &[1.0, 1.0, 1.0], &crate::color::TransferFunction::Rec709,
                         &full, true, pattern).expect("RCD process")
        };
        assert_eq!(gpu_bytes, gpu_bytes2, "RCD dispatches must be bit-deterministic");
        // CPU reference chain, same order as pipeline.rs (Full: no desat).
        let mut rgb = vec![0.0f32; (w * h) as usize * 3];
        BilinearDemosaic::new(pattern).process_par_into(&m, w, 0, 0, w, h, &pattern, &mut rgb).expect("demosaic");
        normalize_linear_per_channel(&mut rgb, 64.0, 64.0, 64.0, 800.0);
        apply_display_rolloff(&mut rgb);
        crate::color::TransferFunction::Rec709.process(&mut rgb);
        let cpu: Vec<u16> = rgb.iter().map(|v| (v.max(0.0) * 65535.0).min(65535.0) as u16).collect();
        let gpu: Vec<u16> = gpu_bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(gpu.len(), cpu.len());
        // Interior only: the outer border ring carries pre-existing RCD
        // edge-transient error (also present before this change), excluded
        // from the agreement bound but included in the band structure.
        let mut maxd = 0i32;
        let mut bands = [0u64; 8]; let mut counts = [0u64; 8];
        for y in 4..h as usize - 4 { for x in 16..112usize {
            let o = (y * w as usize + x) * 3;
            let band = (x - 16) * 8 / 96;
            let deep = y >= 16 && (y as u32) < h - 16 && x >= 32;
            for c in 0..3 {
                let d = (gpu[o + c] as i32 - cpu[o + c] as i32).abs();
                if deep { maxd = maxd.max(d); }
            }
            bands[band] += gpu[o] as u64 + gpu[o + 1] as u64 + gpu[o + 2] as u64; counts[band] += 3;
        }}
        let means: Vec<f64> = bands.iter().zip(counts).map(|(s, c)| *s as f64 / c as f64).collect();
        eprintln!("on-gradient: deep-interior max deviation {maxd} codes, band means {means:.0?}");
        for b in 1..8 {
            assert!(means[b] > means[b - 1], "gradient flattened at band {b}: {means:.0?}");
        }
        assert!(means[7] - means[0] > 15000.0, "gradient erased: {means:.0?}");
        // Interior agreement bound documents pre-existing RCD-vs-bilinear
        // interpolation texture on steep synthetic ramps (zero signed bias:
        // R=+8.8 G=+0.0 B=-4.3 codes deep-interior, i.e. no systematic level
        // error from the desat bypass or the rolloff mirror). The old
        // level-triggered desat flattened bands 4+ (slope ~0) and blew this
        // bound by an order of magnitude.
        assert!(maxd <= 450, "GPU ON must match the clean CPU chain, maxd={maxd}");
    }

    /// ON must NOT desaturate: a chromatic bright field (no censored
    /// photosites, so Full completion passes it through) must keep its
    /// channel ratios through the ON entry. The old unconditional desat
    /// collapsed this to neutral (gap ~0); the fixed path preserves it.
    #[test]
    fn rcd_on_preserves_chroma_no_desat() {
        use crate::file::BayerPattern;
        use crate::hl::{HlParams, HlPolicy};
        let ctx = match pollster::block_on(super::GpuContext::new()) {
            Ok(c) => std::sync::Arc::new(c),
            Err(_) => { eprintln!("SKIP: no GPU adapter"); return; }
        };
        let (w, h) = (64u32, 48u32);
        let pattern = BayerPattern::GBRG;
        // Chromatic bright, all below the HL rail: R/B hot, G dimmer.
        let mut mosaic = vec![0u16; (w * h) as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let c = crate::hl::color_at(x as i32, y as i32, pattern);
                mosaic[y * w as usize + x] = if c == 1 { 800 } else { 1000 };
            }
        }
        let full = HlParams::new(HlPolicy::Full, 1023.0, [64.0, 64.0, 64.0], [1.0, 1.0, 1.0]);
        let mut pipe = super::RcdPipeline::new(ctx, w, h).expect("RcdPipeline::new");
        let bytes = pipe.process(&mosaic, 0x49494949, 64.0, 64.0, 64.0, 1023.0, w, 0, 0,
            &crate::color::identity_ccm(), &[1.0, 1.0, 1.0],
            &crate::color::TransferFunction::Rec709, &full, true, pattern).expect("process");
        let u16s: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        // Interior green-gap must still show the input imbalance. Normalized
        // input gap = 0.9761-0.7675 = 0.2086, which the Rec.709 OETF
        // compresses to ~7300 codes; the old desat (t=1 at m>=1.0) would
        // print ~0. Demosaic texture accounts for single-digit codes.
        let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64); let mut n = 0u64;
        for y in 16..h as usize - 16 { for x in 16..w as usize - 16 {
            let o = (y * w as usize + x) * 3;
            sr += u16s[o] as u64; sg += u16s[o + 1] as u64; sb += u16s[o + 2] as u64; n += 1;
        }}
        let gap = ((sr + sb) as f64 / 2.0 - sg as f64) / n as f64;
        eprintln!("on-chroma: interior green-gap {gap:.0} codes (input shape ~7300)");
        assert!(gap > 5000.0, "ON path desaturated chromatic highlights: gap={gap:.0}");
    }

    /// ON/display hedge parity: an imbalanced above-rail field (censored R/B
    /// completed to the rail, dim G — the warm-sky residual shape) must be
    /// eased toward luminance through the ON entry, matching the CPU
    /// `apply_on_highlight_imbalance_hedge` chain within 3 LSB. Pre-fix the
    /// ON entry had no hedge (gap ~12000 codes); the fixed path prints a
    /// partial residual (~2900 — the lower bound pins that the gate is
    /// partial, not a nuke-to-zero). Interior only (RCD edges).
    #[test]
    fn on_hedge_parity_on_adapter() {
        use crate::file::BayerPattern;
        use crate::hl::{HlGeometry, HlParams, HlPolicy, HlScratch};
        let ctx = match pollster::block_on(super::GpuContext::new()) {
            Ok(c) => std::sync::Arc::new(c),
            Err(_) => { eprintln!("SKIP on_hedge_parity_on_adapter: no GPU adapter"); return; }
        };
        let (w, h) = (64u32, 48u32);
        let pattern = BayerPattern::GBRG;
        let mut mosaic = vec![0u16; (w * h) as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let c = crate::hl::color_at(x as i32, y as i32, pattern);
                mosaic[y * w as usize + x] = if c == 1 { 700 } else { 1800 };
            }
        }
        let full = HlParams::new(HlPolicy::Full, 1023.0, [64.0, 64.0, 64.0], [1.0, 1.0, 1.0]);
        let mut pipe = super::RcdPipeline::new(ctx, w, h).expect("RcdPipeline::new");
        let bytes = pipe.process(&mosaic, 0x49494949, 64.0, 64.0, 64.0, 1023.0, w, 0, 0,
            &crate::color::identity_ccm(), &[1.0, 1.0, 1.0],
            &crate::color::TransferFunction::Rec709, &full, true, pattern).expect("process");
        let u16s: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        // CPU reference: the HL pass floors censored R/B to exactly the rail
        // (support ratio 0.663 < 1 -> est = max(0.663, 1.0) = 1.0 -> DN 1023);
        // flat field keeps both demosaics exact, isolating the hedge stage.
        let mut m2 = mosaic.clone();
        let g = HlGeometry { stride: w as usize, offset_x: 0, offset_y: 0,
                             width: w as usize, height: h as usize, pattern };
        HlScratch::new().apply(&mut m2, &g, &full);
        assert_eq!(m2[1], 1023, "censored B must complete to exactly the rail");
        let norm = |v: f32| (v - 64.0) / 959.0;
        let mut cref = [norm(1023.0), norm(700.0), norm(1023.0)];
        crate::color::apply_on_highlight_imbalance_hedge(&mut cref);
        crate::color::TransferFunction::Rec709.process(&mut cref);
        let cexp = cref.map(|v| (v.clamp(0.0, 1.0) * 65535.0) as u16);
        let mut maxd = 0i32;
        let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64); let mut n = 0u64;
        for y in 4..h as usize - 4 {
            for x in 4..w as usize - 4 {
                let px = &u16s[(y * w as usize + x) * 3..][..3];
                maxd = maxd.max((px[0] as i32 - cexp[0] as i32).abs())
                    .max((px[1] as i32 - cexp[1] as i32).abs())
                    .max((px[2] as i32 - cexp[2] as i32).abs());
                sr += px[0] as u64; sg += px[1] as u64; sb += px[2] as u64; n += 1;
            }
        }
        let gap = ((sr + sb) as f64 / 2.0 - sg as f64) / n as f64;
        eprintln!("on-hedge: max deviation {maxd} codes, residual green-gap {gap:.0} codes (want ~2900)");
        assert!(gap < 5000.0, "ON hedge did not fire on imbalanced rail residual: gap={gap:.0}");
        assert!(gap > 500.0, "ON hedge nuked the residual instead of easing it: gap={gap:.0}");
        assert!(maxd <= 3, "CPU/GPU ON hedge must agree, maxd={maxd}");
    }

    /// ON-log must NOT desaturate: the log soft-clip (luminance-anchored,
    /// `t_soft` up to ~0.9 on completed highlights) has no CPU counterpart
    /// and collapses highlight chroma (measured 12x loss on real frames).
    /// Full policy with `run_hl=false` isolates the fill stage: the mosaic
    /// passes through untouched, so this tests the soft-clip gate alone.
    #[test]
    fn rcd_on_log_preserves_chroma_no_soft_clip() {
        use crate::file::BayerPattern;
        use crate::hl::{HlParams, HlPolicy};
        let ctx = match pollster::block_on(super::GpuContext::new()) {
            Ok(c) => std::sync::Arc::new(c),
            Err(_) => { eprintln!("SKIP: no GPU adapter"); return; }
        };
        let (w, h) = (64u32, 48u32);
        let pattern = BayerPattern::GBRG;
        // Chromatic ABOVE-rail field: soft-clip trigger zone. R/B hot,
        // G dimmer; all censored so content is highlight by construction.
        let mut mosaic = vec![0u16; (w * h) as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let c = crate::hl::color_at(x as i32, y as i32, pattern);
                mosaic[y * w as usize + x] = if c == 1 { 700 } else { 1800 };
            }
        }
        let full = HlParams::new(HlPolicy::Full, 1023.0, [64.0, 64.0, 64.0], [1.0, 1.0, 1.0]);
        let mut pipe = super::RcdPipeline::new(ctx, w, h).expect("RcdPipeline::new");
        let bytes = pipe.process(&mosaic, 0x49494949, 64.0, 64.0, 64.0, 1023.0, w, 0, 0,
            &crate::color::identity_ccm(), &[1.0, 1.0, 1.0],
            &crate::color::TransferFunction::ARRIlog3, &full, false, pattern).expect("process");
        let u16s: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        // Normalized input gap = 1.8096-0.6629 = 1.1467 linear, which the
        // LogC3 OETF compresses to ~7000 codes. The ungated soft-clip
        // (t=0.81 at m=1.81) would print ~1300. Interior only (RCD edges).
        let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64); let mut n = 0u64;
        for y in 16..h as usize - 16 { for x in 16..w as usize - 16 {
            let o = (y * w as usize + x) * 3;
            sr += u16s[o] as u64; sg += u16s[o + 1] as u64; sb += u16s[o + 2] as u64; n += 1;
        }}
        let gap = ((sr + sb) as f64 / 2.0 - sg as f64) / n as f64;
        eprintln!("on-log-chroma: interior green-gap {gap:.0} codes (want ~7000)");
        assert!(gap > 4000.0, "ON-log soft-clip desaturated highlights: gap={gap:.0}");
    }

    /// OFF/basic highlight handling parity: a flat imbalanced-bright field
    /// through the full GPU `process` (Sensor policy -> `main_basic`, Linear
    /// transfer, identity CCM/WB) must come out neutral, matching the CPU
    /// `apply_basic_highlight_desat` chain. Flat input keeps both demosaics
    /// exact, isolating the desat stage. Contract: output neutral (gap < 3
    /// codes) and CPU/GPU agree within 2 LSB.
    #[test]
    fn basic_desat_off_parity_on_adapter() {
        use crate::file::BayerPattern;
        use crate::hl::{HlParams, HlPolicy};
        let ctx = match pollster::block_on(super::GpuContext::new()) {
            Ok(c) => std::sync::Arc::new(c),
            Err(_) => { eprintln!("SKIP basic_desat_off_parity_on_adapter: no GPU adapter"); return; }
        };
        let (w, h) = (64u32, 48u32);
        let pattern = BayerPattern::GBRG;
        // Imbalanced bright: R/B at rail, G below — the sensor-magenta shape.
        let mut mosaic = vec![0u16; (w * h) as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let c = crate::hl::color_at(x as i32, y as i32, pattern);
                mosaic[y * w as usize + x] = if c == 1 { 850 } else { 1023 };
            }
        }
        let sensor = HlParams::new(HlPolicy::Sensor, 1023.0, [64.0, 64.0, 64.0], [1.0, 1.0, 1.0]);
        let mut pipe = super::RcdPipeline::new(ctx, w, h).expect("RcdPipeline::new");
        let bytes = pipe.process(&mosaic, 0x49494949, 64.0, 64.0, 64.0, 1023.0, w, 0, 0,
            &crate::color::identity_ccm(), &[1.0, 1.0, 1.0],
            &crate::color::TransferFunction::Linear, &sensor, false, pattern).expect("process");
        assert_eq!(bytes.len(), (w * h) as usize * 6);
        let u16s: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        // CPU reference through the same stages (flat field: demosaic exact).
        let norm = |v: f32| (v - 64.0) / 959.0;
        let mut cref = [norm(1023.0), norm(850.0), norm(1023.0)];
        crate::color::apply_basic_highlight_desat(&mut cref);
        let cexp = cref.map(|v| (v.clamp(0.0, 1.0) * 65535.0) as u16);
        let mut maxd = 0i32;
        let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
        // Skip a 4px border: RCD edge interpolation is undefined there
        // (pre-existing, both entries); the desat question is interior.
        for y in 4..h as usize - 4 {
            for x in 4..w as usize - 4 {
                let px = &u16s[(y * w as usize + x) * 3..][..3];
            maxd = maxd.max((px[0] as i32 - cexp[0] as i32).abs())
                .max((px[1] as i32 - cexp[1] as i32).abs())
                .max((px[2] as i32 - cexp[2] as i32).abs());
            sr += px[0] as u64; sg += px[1] as u64; sb += px[2] as u64;
            }
        }
        let n = (w as usize - 8) as u64 * (h as usize - 8) as u64;
        let gap = ((sr + sb) as f64 / 2.0 - sg as f64) / n as f64 / 257.0;
        eprintln!("basic desat parity: max deviation {maxd} codes, output green-gap {gap:.2} codes");
        assert!(gap.abs() < 3.0, "OFF output must be neutral, gap={gap:.2}");
        assert!(maxd <= 2, "CPU/GPU OFF desat must agree, maxd={maxd}");
    }

    /// CPU<->GPU completion parity on a real adapter. Skips (passes
    /// vacuously) where no adapter exists. GBRG exercises the non-RGGB
    /// shader arms; the contract is <= 1 LSB per photosite (GPU division is
    /// 2.5 ULP vs correctly rounded x86, so exact equality is not promised),
    /// with the exact-match rate reported.
    #[test]
    fn hl_cpu_gpu_parity_on_adapter() {
        use crate::hl::{HlGeometry, HlParams, HlPolicy, HlScratch};
        let ctx = match pollster::block_on(super::GpuContext::new()) {
            Ok(c) => std::sync::Arc::new(c),
            Err(_) => {
                eprintln!("SKIP hl_cpu_gpu_parity_on_adapter: no GPU adapter");
                return;
            }
        };
        let (w, h) = (64u32, 48u32);
        let pattern = BayerPattern::GBRG;
        let params = HlParams::new(HlPolicy::Full, 1023.0, [64.0, 64.0, 64.0], PARITY_NEUTRAL);
        let geom = HlGeometry { stride: w as usize, offset_x: 0, offset_y: 0, width: w as usize, height: h as usize, pattern };
        // Case 1: clipped frame.
        let mosaic = parity_mosaic(w as usize, h as usize, pattern, true);
        let mut cpu = mosaic.clone();
        let (_, rewritten) = HlScratch::new().apply(&mut cpu, &geom, &params);
        assert!(rewritten > 0, "fixture must rewrite something");
        let mut pipe = super::RcdPipeline::new(ctx, w, h).expect("RcdPipeline::new");
        let gpu = pipe.hl_complete_for_test(&mosaic, &params, pattern).expect("HL dispatch");
        assert_eq!(gpu.len(), cpu.len());
        let mut exact = 0u64;
        let mut off_by_one = 0u64;
        for (i, (&g, &c)) in gpu.iter().zip(cpu.iter()).enumerate() {
            let d = (g as i32 - c as i32).abs();
            assert!(d <= 1, "photosite {i}: gpu={g} cpu={c}");
            if d == 0 {
                exact += 1;
            } else {
                off_by_one += 1;
            }
        }
        eprintln!("hl parity: {exact}/{} exact, {off_by_one} off-by-one", cpu.len());
        for (i, &before) in mosaic.iter().enumerate() {
            if (before as f32) < params.rail_dn {
                assert_eq!(gpu[i], before, "uncensored {i} modified");
            }
        }
        // Case 2: zero-clip frame is bit-exact on both backends.
        let clean = parity_mosaic(w as usize, h as usize, pattern, false);
        let mut cpu_clean = clean.clone();
        let (_, n) = HlScratch::new().apply(&mut cpu_clean, &geom, &params);
        assert_eq!(n, 0);
        let gpu_clean = pipe.hl_complete_for_test(&clean, &params, pattern).expect("HL dispatch clean");
        assert_eq!(gpu_clean, clean, "zero-clip GPU output must be bit-exact");
        // Case 3: Sensor policy is the identity on the GPU.
        let sensor = HlParams::new(HlPolicy::Sensor, 1023.0, [64.0, 64.0, 64.0], PARITY_NEUTRAL);
        let gpu_sensor = pipe.hl_complete_for_test(&mosaic, &sensor, pattern).expect("HL dispatch sensor");
        assert_eq!(gpu_sensor, mosaic, "Sensor GPU output must be bit-exact");
    }
}
