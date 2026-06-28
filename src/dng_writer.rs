use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use tiff::encoder::{Rational, SRational};
use tiff::encoder::TiffEncoder;
use tiff::tags::Tag;

use crate::file::{BayerPattern, McrawFileInfo};
use crate::decoder::LensShadingMap;

// ---------------------------------------------------------------------------
// LJ92 — pure-Rust lossless JPEG (SOF3, predictor 1, 16-bit CFA)
// ---------------------------------------------------------------------------

/// Extended DC luminance Huffman table for categories 0–16 (16-bit data).
/// Standard JPEG Table K.3 extended for categories 12–16.
const DC_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
const DC_HUFFVAL: [u8; 17] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

struct BitWriter {
    buf: Vec<u8>,
    acc: u32,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { buf: Vec::new(), acc: 0, nbits: 0 }
    }

    fn write_bits(&mut self, value: u32, count: u32) {
        self.acc = (self.acc << count) | value;
        self.nbits += count;
        while self.nbits >= 8 {
            let byte = (self.acc >> (self.nbits - 8)) as u8;
            self.buf.push(byte);
            self.nbits -= 8;
        }
        self.acc &= (1 << self.nbits) - 1;
    }

    fn flush(&mut self) {
        if self.nbits > 0 {
            let byte = (self.acc << (8 - self.nbits)) as u8;
            self.buf.push(byte);
            self.nbits = 0;
            self.acc = 0;
        }
    }

    fn byte_stuff(&mut self) {
        let mut i = 0;
        while i < self.buf.len() {
            if self.buf[i] == 0xFF {
                self.buf.insert(i + 1, 0x00);
                i += 2;
            } else {
                i += 1;
            }
        }
    }

    fn into_bytes(mut self) -> Vec<u8> {
        self.flush();
        self.byte_stuff();
        self.buf
    }
}

fn build_huffman_table(bits: &[u8; 16], huffval: &[u8]) -> (Vec<u32>, Vec<u8>) {
    let num_sym = huffval.len();
    let mut codes = vec![0u32; num_sym];
    let mut sizes = vec![0u8; num_sym];
    let mut code: u32 = 0;
    let mut si: usize = 0;
    for i in 0..16 {
        let count = bits[i] as usize;
        for _ in 0..count {
            let sym = huffval[si] as usize;
            codes[sym] = code;
            sizes[sym] = (i + 1) as u8;
            code += 1;
            si += 1;
        }
        code <<= 1;
    }
    (codes, sizes)
}

fn encode_jpeg_diff(diff: i32) -> (u32, u32) {
    if diff == 0 {
        return (0, 0);
    }
    let abs_diff = diff.unsigned_abs();
    let cat = 32 - abs_diff.leading_zeros();
    let extra = if diff > 0 {
        diff as u32
    } else {
        (diff - 1) as u32
    };
    (cat, extra & ((1 << cat) - 1))
}

fn write_diff(bw: &mut BitWriter, diff: i32, codes: &[u32], sizes: &[u8]) {
    let (cat, extra) = encode_jpeg_diff(diff);
    let sym = cat as usize;
    bw.write_bits(codes[sym], sizes[sym] as u32);
    if cat > 0 {
        bw.write_bits(extra, cat);
    }
}

/// Compress 16-bit Bayer CFA data using LJ92 (lossless JPEG, predictor 1).
pub fn compress_lj92(bayer: &[u16], width: usize, height: usize) -> Vec<u8> {
    let (codes, sizes) = build_huffman_table(&DC_BITS, &DC_HUFFVAL);
    let mut bw = BitWriter::new();

    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let predictor = if x == 0 { 0u16 } else { bayer[row + x - 1] };
            let diff = bayer[row + x] as i32 - predictor as i32;
            write_diff(&mut bw, diff, &codes, &sizes);
        }
    }

    let entropy = bw.into_bytes();

    let mut jpeg = Vec::with_capacity(64 + entropy.len());

    // SOI
    jpeg.extend_from_slice(&[0xFF, 0xD8]);

    // SOF3 — lossless frame header (11 bytes after marker)
    jpeg.extend_from_slice(&[0xFF, 0xC3, 0x00, 0x0B, 0x10]);
    jpeg.extend_from_slice(&(height as u16).to_be_bytes());
    jpeg.extend_from_slice(&(width as u16).to_be_bytes());
    jpeg.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);

    // DHT — DC luminance table for categories 0–16
    let dht_len: u16 = 2 + 1 + 16 + 17;
    jpeg.extend_from_slice(&[0xFF, 0xC4]);
    jpeg.extend_from_slice(&dht_len.to_be_bytes());
    jpeg.push(0x00); // DC table 0
    jpeg.extend_from_slice(&DC_BITS);
    jpeg.extend_from_slice(&DC_HUFFVAL);

    // SOS — start of scan (predictor 1, 1 component, DC table 0)
    jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x01, 0x00, 0x00]);

    // Entropy-coded data (already byte-stuffed)
    jpeg.extend_from_slice(&entropy);

    // EOI
    jpeg.extend_from_slice(&[0xFF, 0xD9]);

    jpeg
}

// ---------------------------------------------------------------------------
// DNG helpers
// ---------------------------------------------------------------------------

fn pattern_to_cfa(pattern: BayerPattern, offset_x: u32, offset_y: u32) -> [u8; 4] {
    let mut cfa = match pattern {
        BayerPattern::RGGB => [0u8, 1, 1, 2],
        BayerPattern::GRBG => [1u8, 0, 2, 1],
        BayerPattern::GBRG => [1u8, 2, 0, 1],
        BayerPattern::BGGR => [2u8, 1, 1, 0],
        _ => [0u8, 1, 1, 2],
    };
    if offset_x & 1 == 1 {
        cfa = [cfa[1], cfa[0], cfa[3], cfa[2]];
    }
    if offset_y & 1 == 1 {
        cfa = [cfa[2], cfa[3], cfa[0], cfa[1]];
    }
    cfa
}

fn stem_from_path(path: &str) -> String {
    let base = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("clip")
        .to_string();
    if base.ends_with("-metadata") {
        base.trim_end_matches("-metadata").to_string()
    } else {
        base
    }
}

fn f64_to_srational(v: f64) -> SRational {
    SRational { n: (v * 10000.0).round() as i32, d: 10000 }
}

fn f32_to_srational(v: f32) -> SRational {
    SRational { n: (v * 10000.0).round() as i32, d: 10000 }
}

fn matrix9_as_srational_slice(m: &[f64; 9]) -> [SRational; 9] {
    [
        f64_to_srational(m[0]), f64_to_srational(m[1]), f64_to_srational(m[2]),
        f64_to_srational(m[3]), f64_to_srational(m[4]), f64_to_srational(m[5]),
        f64_to_srational(m[6]), f64_to_srational(m[7]), f64_to_srational(m[8]),
    ]
}

fn asn_as_srational_slice(asn: [f32; 3]) -> [SRational; 3] {
    [f32_to_srational(asn[0]), f32_to_srational(asn[1]), f32_to_srational(asn[2])]
}

// ---------------------------------------------------------------------------
// GainMap opcode (DNG OpcodeList2, tag 0xC61B, opcode ID 9)
// ---------------------------------------------------------------------------

/// Build the binary blob for a single GainMap opcode embedded in OpcodeList2.
///
/// Returns the complete byte array for tag 0xC61B including the opcode-count
/// prefix and the `GainMapOpcode` payload per the DNG 1.7 specification.
/// The output is big-endian as required by TIFF/DNG IFD encoding.
pub fn build_gainmap_opcode_blob(
    map: &LensShadingMap,
    top: u32,
    left: u32,
    bottom: u32,
    right: u32,
) -> Vec<u8> {
    use std::io::Write;
    let mut buf = Vec::with_capacity(1024);

    // OpcodeList2 header: opcode count (u32BE)
    buf.write_all(&1u32.to_be_bytes()).unwrap();

    // OpcodeRecord: opcode_id (u16BE), dng_version (u32BE), flags (u32BE),
    //               data_size (u32BE)
    let opcode_id = 9u16; // GainMap
    let dng_version = 1_007_000u32; // DNG 1.7.0.0
    let flags = 0u32; // not optional
    buf.write_all(&opcode_id.to_be_bytes()).unwrap();
    buf.write_all(&dng_version.to_be_bytes()).unwrap();
    buf.write_all(&flags.to_be_bytes()).unwrap();

    // Reserve space for data_size — we'll fill it after computing the payload
    let data_size_offset = buf.len();
    buf.write_all(&0u32.to_be_bytes()).unwrap();

    // GainMap data payload
    let payload_offset = buf.len();

    // GainMapID = 0 (Lens Shading)
    buf.write_all(&0u32.to_be_bytes()).unwrap();

    // Top / Left / Bottom / Right
    buf.write_all(&top.to_be_bytes()).unwrap();
    buf.write_all(&left.to_be_bytes()).unwrap();
    buf.write_all(&bottom.to_be_bytes()).unwrap();
    buf.write_all(&right.to_be_bytes()).unwrap();

    // MapPlanes = 4 (R, G1, G2, B)
    buf.write_all(&4u32.to_be_bytes()).unwrap();

    // MapWidth / MapHeight
    buf.write_all(&map.width.to_be_bytes()).unwrap();
    buf.write_all(&map.height.to_be_bytes()).unwrap();

    // MapType = 0 (f32 map)
    buf.write_all(&0u32.to_be_bytes()).unwrap();

    // MapFlags = 1 (BilinearInterpolate)
    buf.write_all(&1u32.to_be_bytes()).unwrap();

    // Map data: f32 × map_width × map_height × 4 planes, big-endian
    for plane_idx in 0..4 {
        let plane = &map.channels[plane_idx];
        for &gain in plane {
            buf.write_all(&gain.to_be_bytes()).unwrap();
        }
    }

    // Go back and fill in data_size
    let data_size = (buf.len() - payload_offset) as u32;
    let data_size_bytes = data_size.to_be_bytes();
    buf[data_size_offset..data_size_offset + 4].copy_from_slice(&data_size_bytes);

    buf
}

// ---------------------------------------------------------------------------
// DngWriter
// ---------------------------------------------------------------------------

/// Writes a single CinemaDNG frame with LJ92 compression and full DNG tags.
pub struct DngWriter<'a> {
    info: &'a McrawFileInfo,
    bayer: &'a [u16],
    as_shot_neutral: [f32; 3],
    frame_index: usize,
}

impl<'a> DngWriter<'a> {
    pub fn new(
        info: &'a McrawFileInfo,
        bayer: &'a [u16],
        as_shot_neutral: [f32; 3],
        frame_index: usize,
    ) -> Self {
        DngWriter { info, bayer, as_shot_neutral, frame_index }
    }

    /// Generate the output filename (CinemaDNG zero‑padded).
    pub fn filename(&self, output_dir: &str) -> String {
        let stem = stem_from_path(&self.info.path);
        std::path::Path::new(output_dir)
            .join(format!("{}_{:06}.dng", stem, self.frame_index))
            .to_string_lossy()
            .to_string()
    }

    /// Write the DNG file to disk.
    pub fn write_to_path<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();
        let width = self.info.width as usize;
        let height = self.info.height as usize;
        let pixel_count = width * height;

        if self.bayer.len() < pixel_count {
            anyhow::bail!(
                "Bayer buffer too short for {}×{}: got {} pixels, need {}",
                width, height, self.bayer.len(), pixel_count,
            );
        }

        // ---- Compress with LJ92 ----
        let compressed = compress_lj92(self.bayer, width, height);

        // ---- Write TIFF/DNG via tiff crate ----
        let file = std::fs::File::create(path)
            .with_context(|| format!("Failed to create DNG file: {}", path.display()))?;
        let mut writer = BufWriter::new(file);
        let mut tiff = TiffEncoder::new(&mut writer)
            .context("Failed to create TIFF encoder")?;
        let mut dir = tiff.new_directory()
            .context("Failed to start TIFF directory")?;

        // Write LJ92 compressed data first — returns its file offset
        let data_off = dir.write_data(&compressed[..])
            .context("Failed to write LJ92 compressed data")?;

        // ---- Write inline IFD entries ----
        let aw = width as u16;
        let ah = height as u16;
        dir.write_tag(Tag::Unknown(256), aw)?;   // ImageWidth
        dir.write_tag(Tag::Unknown(257), ah)?;   // ImageLength
        dir.write_tag(Tag::Unknown(258), 16u16)?; // BitsPerSample
        dir.write_tag(Tag::Unknown(259), 7u16)?;  // Compression (Lossless JPEG)
        dir.write_tag(Tag::Unknown(262), 32803u16)?; // PhotometricInterpretation (CFA)
        dir.write_tag(Tag::Unknown(273), data_off as u32)?; // StripOffsets
        dir.write_tag(Tag::Unknown(277), 1u16)?;  // SamplesPerPixel
        dir.write_tag(Tag::Unknown(278), ah as u32)?; // RowsPerStrip
        dir.write_tag(Tag::Unknown(279), compressed.len() as u32)?; // StripByteCounts

        // CFA pattern
        let cfa_off_x = self.info.active_offset_x as u32;
        let cfa_off_y = self.info.active_offset_y as u32;
        let cfa = pattern_to_cfa(self.info.bayer_pattern, cfa_off_x, cfa_off_y);
        dir.write_tag(Tag::Unknown(33421), &[0u16, 1, 2][..])?;
        dir.write_tag(Tag::Unknown(33422), &cfa[..])?;

        dir.write_tag(Tag::Unknown(50706), &[1u8, 4, 0, 0][..])?;
        dir.write_tag(Tag::Unknown(50707), &[1u8, 4, 0, 0][..])?;

        // Unique camera model (ASCII)
        let model = self.info.camera_metadata.camera_model
            .as_deref()
            .unwrap_or("Unknown");
        dir.write_tag(Tag::Unknown(50708), model.as_bytes())?;

        // ColorMatrix1 — SRATIONAL[9]; > 4 bytes so stored at next file pos
        let cm1 = self.info.camera_metadata.color_matrix
            .unwrap_or([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        let cm1_arr = matrix9_as_srational_slice(&cm1);
        dir.write_tag(Tag::Unknown(50721), &cm1_arr[..])?;

        // Calibration illuminants
        let ill1 = self.info.camera_metadata.calibration_illuminant1.unwrap_or(21);
        dir.write_tag(Tag::Unknown(50778), ill1 as u16)?;
        if let Some(ill2) = self.info.camera_metadata.calibration_illuminant2 {
            dir.write_tag(Tag::Unknown(50779), ill2 as u16)?;
        }

        // ColorMatrix2 (if available)
        if let Some(ref cm2) = self.info.camera_metadata.color_matrix2 {
            let cm2_arr = matrix9_as_srational_slice(cm2);
            dir.write_tag(Tag::Unknown(50722), &cm2_arr[..])?;
        }

        // AsShotNeutral — SRATIONAL[3]
        let asn_arr = asn_as_srational_slice(self.as_shot_neutral);
        dir.write_tag(Tag::Unknown(50728), &asn_arr[..])?;

        // BlackLevel — SHORT[4] repeating 2×2 pattern
        let bl = self.info.black_level as u16;
        dir.write_tag(Tag::Unknown(50714), &[bl, bl, bl, bl][..])?;

        // WhiteLevel
        dir.write_tag(Tag::Unknown(50717), self.info.white_level as u32)?;

        // OpcodeList2 — GainMap opcode for lens shading correction
        if let Some(ref shading) = self.info.lens_shading_map {
            let at = self.info.active_offset_y as u32;
            let al = self.info.active_offset_x as u32;
            let ab = at + self.info.active_height as u32;
            let ar = al + self.info.active_width as u32;
            let opcode_blob = build_gainmap_opcode_blob(shading, at, al, ab, ar);
            dir.write_tag(Tag::Unknown(0xC61B), &opcode_blob[..])?;
        }

        // DefaultScale — RATIONAL[2] = [1, 1]
        dir.write_tag(Tag::Unknown(50718), &[
            Rational { n: 1, d: 1 },
            Rational { n: 1, d: 1 },
        ][..])?;

        // DefaultCropOrigin — RATIONAL[2] = [left, top]
        dir.write_tag(Tag::Unknown(50719), &[
            Rational { n: self.info.active_offset_x as u32, d: 1 },
            Rational { n: self.info.active_offset_y as u32, d: 1 },
        ][..])?;

        // DefaultCropSize — RATIONAL[2] = [active_w, active_h]
        dir.write_tag(Tag::Unknown(50720), &[
            Rational { n: self.info.active_width as u32, d: 1 },
            Rational { n: self.info.active_height as u32, d: 1 },
        ][..])?;

        // ActiveArea — LONG[4] = [top, left, bottom, right]
        let at = self.info.active_offset_y as u32;
        let al = self.info.active_offset_x as u32;
        let ab = at + self.info.active_height as u32;
        let ar = al + self.info.active_width as u32;
        dir.write_tag(Tag::Unknown(0xC68E), &[at, al, ab, ar][..])?;

        // ---- Finalize ----
        dir.finish().context("Failed to finalize TIFF directory")?;
        writer.flush().context("Failed to flush DNG file")?;

        Ok(())
    }
}
