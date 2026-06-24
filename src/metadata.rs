use ratatui::style::Color;
use ratatui::text::{Line, Span};

use crate::file::McrawFileInfo;

pub fn format_metadata_for_display(info: &McrawFileInfo) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.extend(format_general_section(info));
    lines.extend(format_video_section(info));
    lines.extend(format_camera_section(info));
    lines.extend(format_audio_section(info));
    lines
}

pub fn format_general_section(info: &McrawFileInfo) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "General",
        Color::Yellow,
    )));

    let filename = info
        .path
        .split('/')
        .last()
        .unwrap_or(&info.path);
    lines.push(Line::from(format!(
        "  Filename:     {}",
        filename
    )));
    lines.push(Line::from(format!("  Path:         {}", info.path)));
    lines.push(Line::from(format!("  Size:         {}", format_size(info.size))));
    lines.push(Line::from(format!(
        "  Format:       {}",
        info.format_name()
    )));

    if let Some(ref date) = info.camera_metadata.capture_date {
        lines.push(Line::from(format!(
            "  Capture Date: {}",
            format_capture_date(date)
        )));
    }

    lines.push(Line::from(""));
    lines
}

pub fn format_camera_section(info: &McrawFileInfo) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Camera",
        Color::Yellow,
    )));

    if let Some(ref model) = info.camera_metadata.camera_model {
        if !model.is_empty() {
            lines.push(Line::from(format!("  Camera:       {}", model)));
        }
    }
    if let Some(ref lens) = info.camera_metadata.lens_model {
        lines.push(Line::from(format!("  Lens:         {}", lens)));
    }
    if let Some(fl) = info.camera_metadata.focal_length {
        lines.push(Line::from(format!("  Focal Length: {:.1}mm", fl)));
    }
    if let Some(ap) = info.camera_metadata.aperture {
        lines.push(Line::from(format!("  Aperture:     f/{:.1}", ap)));
    }
    if let Some(iso) = info.camera_metadata.iso {
        lines.push(Line::from(format!("  ISO:          {}", iso)));
    }
    if let Some(et) = info.camera_metadata.exposure_time {
        lines.push(Line::from(format!(
            "  Exposure:     {}",
            format_exposure_time(et)
        )));
    }
    if let Some(wb) = info.camera_metadata.white_balance {
        lines.push(Line::from(format!("  White Balance:{:.0}K", wb)));
    }
    if let Some(ref cm) = info.camera_metadata.color_matrix {
        let vals: Vec<String> = cm.iter().map(|v| format!("{:.2}", v)).collect();
        lines.push(Line::from(format!("  Color Matrix1: [{}]", vals.join(", "))));
    }
    if let Some(ref cm) = info.camera_metadata.color_matrix2 {
        let vals: Vec<String> = cm.iter().map(|v| format!("{:.2}", v)).collect();
        lines.push(Line::from(format!("  Color Matrix2: [{}]", vals.join(", "))));
    }
    if let Some(i1) = info.camera_metadata.calibration_illuminant1 {
        if let Some(i2) = info.camera_metadata.calibration_illuminant2 {
            lines.push(Line::from(format!("  Cal Illuminants: {} / {}", i1, i2)));
        }
    }

    lines.push(Line::from(""));
    lines
}

pub fn format_video_section(info: &McrawFileInfo) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Video",
        Color::Yellow,
    )));

    lines.push(Line::from(format!(
        "  Resolution:   {}x{} ({})",
        info.width, info.height, info.resolution_label()
    )));
    lines.push(Line::from(format!("  FPS:          {:.2}", info.fps)));
    lines.push(Line::from(format!(
        "  Duration:     {}",
        format_duration(info.duration_seconds())
    )));
    lines.push(Line::from(format!("  Frames:       {}", info.frame_count)));
    lines.push(Line::from(format!(
        "  Bit Depth:    {}-bit",
        info.bit_depth
    )));
    lines.push(Line::from(format!(
        "  Bayer:        {}",
        info.bayer_pattern.name()
    )));

    if info.active_width > 0 && info.active_height > 0 {
        lines.push(Line::from(format!(
            "  Active Area:  {}x{} @({},{})",
            info.active_width, info.active_height, info.active_offset_x, info.active_offset_y
        )));
    }

    lines.push(Line::from(""));
    lines
}

pub fn format_audio_section(info: &McrawFileInfo) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Audio",
        Color::Yellow,
    )));

    if info.has_audio {
        lines.push(Line::from("  Has Audio:    Yes".to_string()));
        if info.audio_sample_rate > 0 {
            lines.push(Line::from(format!(
                "  Sample Rate:  {} Hz",
                info.audio_sample_rate
            )));
        }
        if info.audio_channels > 0 {
            let ch_name = if info.audio_channels == 1 {
                "mono"
            } else if info.audio_channels == 2 {
                "stereo"
            } else {
                "multi"
            };
            lines.push(Line::from(format!(
                "  Channels:    {} ({})",
                info.audio_channels, ch_name
            )));
        }
        if let Some(length) = info.audio_length {
            lines.push(Line::from(format!("  Audio Length: {} bytes", length)));
        }
        if let Some(offset) = info.audio_offset {
            lines.push(Line::from(format!("  Audio Offset: {} bytes", offset)));
        }
    } else {
        lines.push(Line::from("  Has Audio:    No".to_string()));
    }

    lines.push(Line::from(""));
    lines
}

pub fn format_duration(seconds: f64) -> String {
    if seconds <= 0.0 {
        return "0:00".to_string();
    }

    let total_secs = seconds as u64;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{}:{:02}", minutes, secs)
    }
}

pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub fn format_exposure_time(value: f64) -> String {
    if value <= 0.0 {
        return "Unknown".to_string();
    }

    let denominator = (1.0 / value).round() as u64;
    if denominator > 0 && denominator <= 10000 {
        format!("1/{}s", denominator)
    } else {
        format!("{:.2}s", value)
    }
}

pub fn format_capture_date(raw: &str) -> String {
    let raw = raw.trim();

    if raw.len() >= 19 {
        let date_part = &raw[..10];
        let time_part = &raw[11..19];
        let tz_part = raw[19..].trim();

        let mut result = format!("{} {}", date_part, time_part);
        if !tz_part.is_empty() {
            result.push_str(tz_part);
        }
        return result;
    }

    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(0.0), "0:00");
        assert_eq!(format_duration(60.0), "1:00");
        assert_eq!(format_duration(120.0), "2:00");
        assert_eq!(format_duration(90.0), "1:30");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3600.0), "1:00:00");
        assert_eq!(format_duration(3725.0), "1:02:05");
        assert_eq!(format_duration(7200.0), "2:00:00");
    }

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1024 * 512), "512.00 KB");
    }

    #[test]
    fn test_format_size_kb() {
        assert_eq!(format_size(1024 * 10), "10.00 KB");
        assert_eq!(format_size(1024 * 1024 - 1), "1024.00 KB");
    }

    #[test]
    fn test_format_size_mb() {
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
        assert_eq!(format_size(1024 * 1024 * 10), "10.00 MB");
        assert_eq!(format_size(1024 * 1024 * 256), "256.00 MB");
    }

    #[test]
    fn test_format_size_gb() {
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_size(1024 * 1024 * 1024 * 2), "2.00 GB");
        assert_eq!(format_size(1024 * 1024 * 1024 * 4), "4.00 GB");
    }

    #[test]
    fn test_format_exposure_time() {
        assert_eq!(format_exposure_time(0.0), "Unknown");
        assert_eq!(format_exposure_time(1.0), "1/1s");
        assert_eq!(format_exposure_time(0.5), "1/2s");
        assert_eq!(format_exposure_time(1.0 / 60.0), "1/60s");
        assert_eq!(format_exposure_time(1.0 / 120.0), "1/120s");
        assert_eq!(format_exposure_time(1.0 / 1000.0), "1/1000s");
    }

    #[test]
    fn test_format_capture_date() {
        assert_eq!(
            format_capture_date("2024-01-15T10:30:45+00:00"),
            "2024-01-15 10:30:45+00:00"
        );
        assert_eq!(
            format_capture_date("2024-06-20T14:00:00-05:00"),
            "2024-06-20 14:00:00-05:00"
        );
        assert_eq!(format_capture_date("raw-date"), "raw-date");
    }
}
