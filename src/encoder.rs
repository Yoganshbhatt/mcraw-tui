use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone)]
pub enum OutputFormat {
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
            OutputFormat::DNG { .. } => "DNG",
            OutputFormat::ProRes { .. } => "ProRes",
            OutputFormat::H264 { .. } => "H.264",
            OutputFormat::HEVC { .. } => "HEVC",
        }
    }

    pub fn output_path(&self) -> Option<&PathBuf> {
        match &self.format {
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
        tracing::info!("encoder stub initialized");
        Encoder {}
    }

    pub async fn start_job(&self, job: EncodeJob) -> Result<()> {
        tracing::info!("[stub] starting encode job: {} -> {:?}", job.id, job.format);
        Ok(())
    }

    pub async fn cancel_job(&self, _job_id: &str) -> Result<()> {
        tracing::info!("[stub] canceling encode job: {}", _job_id);
        Ok(())
    }

    pub async fn list_supported_formats(&self) -> Vec<&'static str> {
        vec!["DNG", "ProRes", "H.264", "HEVC"]
    }
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

pub struct VideoEncoder {
    child: Child,
    audio_temp_path: Option<PathBuf>,
    stderr_log_path: PathBuf,
}

impl VideoEncoder {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        output_path: &str, width: u32, height: u32, fps: f64,
        codec: &str, pix_fmt: &str, extra_args: &[String],
        audio_temp_path: Option<&Path>,
        audio_sample_rate: u32,
        audio_channels: u16,
    ) -> Result<Self> {
        const INPUT_PIX_FMT: &str = "rgb48le";

        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-f", "rawvideo",
            "-pix_fmt", INPUT_PIX_FMT,
            "-s", &format!("{}x{}", width, height),
            "-r", &format!("{}", fps),
            "-i", "-",
        ]);

        // Add audio input from temp file if available
        if let Some(audio_path) = audio_temp_path {
            cmd.args([
                "-f", "s16le",
                "-ar", &audio_sample_rate.to_string(),
                "-ac", &audio_channels.to_string(),
                "-i", &audio_path.to_string_lossy(),
            ]);
        }

        cmd.args([
            "-c:v", codec,
            "-pix_fmt", pix_fmt,
        ]);

        // Append dynamic extra args (profile, crf, preset, color tags,
        // scale filter for wide-gamut YUV conversion, etc.).
        // The scale filter (if needed) is injected by to_ffmpeg_args in
        // export.rs — we pass it through as part of extra_args.
        cmd.args(extra_args);

        // Move the `moov` atom to the front of the file on finalize. Without
        // this, the MP4/MOV muxer writes `moov` after all `mdat` chunks, so
        // players (VLC, mpv, browser <video>) have to scan the whole file
        // before they can seek or start playback. Cost: ~1-2 s at finalize.
        // Harmless for codecs that don't use a moov box (DNG, raw streams).
        if output_path.to_lowercase().ends_with(".mp4")
            || output_path.to_lowercase().ends_with(".mov")
        {
            cmd.args(["-movflags", "+faststart"]);
        }

        // NOTE: VUI signalling (color_primaries / color_trc / colorspace) is
        // passed through `extra_args` and propagates automatically to libx264
        // and libx265. We deliberately do **not** emit hard-coded
        // `-x264-params colorprim=bt709:...` / `-x265-params colorprim=bt709:...`
        // here because it would override the user's chosen gamut/transfer.

        // Add audio encoder if audio input is present
        if audio_temp_path.is_some() {
            let audio_codec = if output_path.to_lowercase().ends_with(".mov") {
                "pcm_s16le"
            } else {
                "aac"
            };
            cmd.args(["-c:a", audio_codec]);
        }

        // Capture FFmpeg stderr to a temp log file so we can surface real
        // errors back to the user. The previous `/dev/null` redirect made
        // failures appear as opaque "FFmpeg stdin not available" messages.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let stderr_log_path = std::env::temp_dir()
            .join(format!("mcraw_ffmpeg_stderr_{}.log", ts));
        let stderr_file = std::fs::File::create(&stderr_log_path)
            .map_err(|e| anyhow::anyhow!("Failed to create ffmpeg stderr log: {}", e))?;

        cmd.arg("-y").arg(output_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_file));

        let child = cmd.spawn()?;
        tracing::info!("ffmpeg subprocess spawned: pid={} codec={} {}x{}@{}fps output={} stderr_log={}",
            child.id(), codec, width, height, fps, output_path, stderr_log_path.display());

        Ok(Self {
            child,
            audio_temp_path: audio_temp_path.map(|p| p.to_path_buf()),
            stderr_log_path,
        })
    }

    /// Read the captured FFmpeg stderr log (best-effort) and return the
    /// final ~2 KB. Used to enrich error messages with the actual reason
    /// FFmpeg failed.
    fn tail_stderr(&self) -> String {
        Self::tail_stderr_from(&self.stderr_log_path)
    }

    /// Free-function variant of `tail_stderr` so callers that already hold
    /// a mutable borrow on `self.child` (e.g. inside `as_mut().ok_or_else(...)`)
    /// can still pull stderr without re-borrowing `self`.
    fn tail_stderr_from(path: &Path) -> String {
        const TAIL_BYTES: usize = 2048;
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => return String::new(),
        };
        let start = bytes.len().saturating_sub(TAIL_BYTES);
        String::from_utf8_lossy(&bytes[start..]).trim().to_string()
    }

    pub fn push_frame(&mut self, data: &[u8]) -> Result<()> {
        use std::io::Write;
        // Capture the stderr-log path up front so the error-handling
        // closure does not need to re-borrow `self` while `self.child.stdin`
        // is being borrowed mutably.
        let stderr_path = self.stderr_log_path.clone();
        let stdin = self.child.stdin.as_mut().ok_or_else(|| {
            tracing::error!("ffmpeg stdin not available");
            let stderr_tail = Self::tail_stderr_from(&stderr_path);
            if stderr_tail.is_empty() {
                anyhow::anyhow!("FFmpeg stdin not available (process may have crashed)")
            } else {
                anyhow::anyhow!("FFmpeg failed:\n{}", stderr_tail)
            }
        })?;
        if let Err(e) = stdin.write_all(data) {
            let stderr_tail = Self::tail_stderr_from(&stderr_path);
            tracing::error!("ffmpeg push_frame error: {} | stderr: {}", e, stderr_tail);
            if stderr_tail.is_empty() {
                return Err(anyhow::anyhow!("FFmpeg write failed: {}", e));
            } else {
                return Err(anyhow::anyhow!("FFmpeg failed:\n{}", stderr_tail));
            }
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        tracing::debug!("ffmpeg finish: closing stdin and waiting");
        drop(self.child.stdin.take());
        let status = self.child.wait()?;
        tracing::info!("ffmpeg subprocess exited: {}", status);
        if status.success() {
            // Successful run — clean up the stderr log.
            let _ = std::fs::remove_file(&self.stderr_log_path);
            Ok(())
        } else {
            let stderr_tail = self.tail_stderr();
            if stderr_tail.is_empty() {
                Err(anyhow::anyhow!("FFmpeg exited with status: {}", status))
            } else {
                Err(anyhow::anyhow!("FFmpeg exited with status {}:\n{}", status, stderr_tail))
            }
        }
    }

    /// Force-terminate the FFmpeg subprocess. Used during cancellation to
    /// unblock a writer thread that may be stuck in `push_frame()`.
    pub fn kill(&mut self) {
        tracing::debug!("ffmpeg kill: terminating subprocess");
        let _ = self.child.stdin.take();
        let _ = self.child.kill();
    }

    /// Returns the OS process ID of the FFmpeg subprocess.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        let _ = self.child.stdin.take();
        let _ = self.child.kill();
        if let Some(ref path) = self.audio_temp_path {
            let _ = std::fs::remove_file(path);
        }
        // Best-effort cleanup; the log may have already been removed by
        // `finish()` on success, or kept by an error path for diagnostics.
        let _ = std::fs::remove_file(&self.stderr_log_path);
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
                id: "2".to_string(),
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
                id: "3".to_string(),
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
                id: "4".to_string(),
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
