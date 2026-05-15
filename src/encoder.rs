use anyhow::Result;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone)]
pub enum OutputFormat {
    CDNG { sequence_dir: PathBuf },
    DNG { output_path: PathBuf },
    ProRes { output_path: PathBuf },
    H264 { output_path: PathBuf },
    HEVC { output_path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeStatus {
    Queued,
    Running,
    Completed,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct EncodeJob {
    pub id: String,
    pub format: OutputFormat,
    pub status: EncodeStatus,
    pub progress: f64,
    pub error: Option<String>,
}

impl EncodeJob {
    pub fn new(id: String, format: OutputFormat) -> Self {
        EncodeJob {
            id,
            format,
            status: EncodeStatus::Queued,
            progress: 0.0,
            error: None,
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(self.status, EncodeStatus::Completed)
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.status, EncodeStatus::Failed(_))
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, EncodeStatus::Running)
    }

    pub fn format_label(&self) -> &'static str {
        match &self.format {
            OutputFormat::CDNG { .. } => "cDNG",
            OutputFormat::DNG { .. } => "DNG",
            OutputFormat::ProRes { .. } => "ProRes",
            OutputFormat::H264 { .. } => "H.264",
            OutputFormat::HEVC { .. } => "HEVC",
        }
    }

    pub fn output_path(&self) -> Option<&PathBuf> {
        match &self.format {
            OutputFormat::CDNG { sequence_dir } => Some(sequence_dir),
            OutputFormat::DNG { output_path } => Some(output_path),
            OutputFormat::ProRes { output_path } => Some(output_path),
            OutputFormat::H264 { output_path } => Some(output_path),
            OutputFormat::HEVC { output_path } => Some(output_path),
        }
    }
}

pub struct Encoder;

impl Encoder {
    pub fn new() -> Self {
        log::info!("Encoder stub initialized");
        Encoder
    }

    pub async fn start_job(&self, job: EncodeJob) -> Result<()> {
        log::info!("[Stub] Starting encode job: {} -> {:?}", job.id, job.format);
        Ok(())
    }

    pub async fn cancel_job(&self, _job_id: &str) -> Result<()> {
        log::info!("[Stub] Canceling encode job: {}", _job_id);
        Ok(())
    }

    pub async fn list_supported_formats(&self) -> Vec<&'static str> {
        vec!["cDNG", "DNG", "ProRes", "H.264", "HEVC"]
    }
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct VideoEncoder {
    child: Child,
}

impl VideoEncoder {
    pub fn new(output_path: &str, width: u32, height: u32, fps: f64, codec: &str, pix_fmt: &str, extra_args: &[&str]) -> Result<Self> {
        const INPUT_PIX_FMT: &str = "rgb48le";

        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-f", "rawvideo",
            "-pix_fmt", INPUT_PIX_FMT,
            "-s", &format!("{}x{}", width, height),
            "-r", &format!("{}", fps),
            "-i", "-",
            "-c:v", codec,
            "-pix_fmt", pix_fmt,
        ]);

        // Append dynamic extra args (profile, crf, preset, etc.)
        cmd.args(extra_args);

        if codec == "libx264" {
            cmd.args(["-x264-params", "colorprim=bt709:transfer=bt709:colormatrix=bt709"]);
        } else if codec == "libx265" {
            cmd.args(["-x265-params", "colorprim=bt709:transfer=bt709:colormatrix=bt709"]);
        } else if codec == "prores_ks" || codec == "prores_videotoolbox" {
            cmd.args(["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709"]);
        }

        cmd.arg("-y").arg(output_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = cmd.spawn()?;

        Ok(Self { child })
    }

    pub fn push_frame(&mut self, data: &[u8]) -> Result<()> {
        use std::io::Write;
        let stdin = self.child.stdin.as_mut().ok_or_else(|| {
            anyhow::anyhow!("FFmpeg stdin not available")
        })?;
        stdin.write_all(data)?;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        drop(self.child.stdin.take());
        self.child.wait()?;
        Ok(())
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        let _ = self.child.stdin.take();
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_job_new() {
        let job = EncodeJob::new(
            "test-1".to_string(),
            OutputFormat::DNG {
                output_path: PathBuf::from("/tmp/test.dng"),
            },
        );
        assert_eq!(job.id, "test-1");
        assert_eq!(job.status, EncodeStatus::Queued);
        assert_eq!(job.progress, 0.0);
        assert_eq!(job.format_label(), "DNG");
    }

    #[test]
    fn test_encode_job_status_checks() {
        let mut job = EncodeJob::new(
            "test-1".to_string(),
            OutputFormat::ProRes {
                output_path: PathBuf::from("/tmp/test.mov"),
            },
        );

        assert!(!job.is_complete());
        assert!(!job.is_failed());
        assert!(!job.is_running());

        job.status = EncodeStatus::Running;
        assert!(job.is_running());
        assert!(!job.is_complete());

        job.status = EncodeStatus::Completed;
        assert!(job.is_complete());
        assert!(!job.is_running());

        job.status = EncodeStatus::Failed("error".to_string());
        assert!(job.is_failed());
    }

    #[test]
    fn test_format_labels() {
        let cdng = OutputFormat::CDNG {
            sequence_dir: PathBuf::from("/tmp/cdng"),
        };
        let dng = OutputFormat::DNG {
            output_path: PathBuf::from("/tmp/dng"),
        };
        let prores = OutputFormat::ProRes {
            output_path: PathBuf::from("/tmp/prores"),
        };
        let h264 = OutputFormat::H264 {
            output_path: PathBuf::from("/tmp/h264"),
        };
        let hevc = OutputFormat::HEVC {
            output_path: PathBuf::from("/tmp/hevc"),
        };

        assert_eq!(
            EncodeJob {
                id: "1".to_string(),
                format: cdng.clone(),
                status: EncodeStatus::Queued,
                progress: 0.0,
                error: None,
            }
            .format_label(),
            "cDNG"
        );
        assert_eq!(
            EncodeJob {
                id: "2".to_string(),
                format: dng,
                status: EncodeStatus::Queued,
                progress: 0.0,
                error: None,
            }
            .format_label(),
            "DNG"
        );
        assert_eq!(
            EncodeJob {
                id: "3".to_string(),
                format: prores,
                status: EncodeStatus::Queued,
                progress: 0.0,
                error: None,
            }
            .format_label(),
            "ProRes"
        );
        assert_eq!(
            EncodeJob {
                id: "4".to_string(),
                format: h264,
                status: EncodeStatus::Queued,
                progress: 0.0,
                error: None,
            }
            .format_label(),
            "H.264"
        );
        assert_eq!(
            EncodeJob {
                id: "5".to_string(),
                format: hevc,
                status: EncodeStatus::Queued,
                progress: 0.0,
                error: None,
            }
            .format_label(),
            "HEVC"
        );
    }
}
