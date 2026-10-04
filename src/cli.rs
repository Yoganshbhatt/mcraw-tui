use clap::{Parser, Subcommand};

#[derive(Subcommand, Debug)]
pub enum CliCommands {
    /// Open a .mcraw file in the TUI
    Open {
        /// Path to the .mcraw file
        #[arg()]
        file: Option<String>,
    },
    /// Show file metadata and exit
    Info {
        /// Path to the .mcraw file
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Export a .mcraw file to another format
    Export {
        /// Path to the .mcraw file
        #[arg(short, long)]
        file: Option<String>,
        /// Export format: dng, prores, dnxhr, h264, hevc, av1, vp9
        #[arg(short = 'F', long)]
        format: String,
        /// Output path or directory
        #[arg(short, long)]
        output: String,
        /// Color space for export (e.g. "Rec.709", "ARRI Wide Gamut 4", "DaVinci Wide Gamut")
        #[arg(long, default_value = "Rec.709")]
        color_space: String,
        /// Transfer function (e.g. "Rec.709", "ARRI LogC4", "S-Log3", "Gamma 2.4")
        #[arg(long, default_value = "Rec.709")]
        transfer_function: String,
        /// ProRes profile: Proxy, LT, Standard, HQ, P4444, XQ4444
        #[arg(long, default_value = "HQ")]
        prores_profile: String,
        /// DNxHR profile: SQ, HD, HDX, HQX, P444
        #[arg(long, default_value = "HQX")]
        dnxhr_profile: String,
        /// HEVC profile ("Main 10 4:2:0", "Main 10 4:4:4")
        #[arg(long, default_value = "Main 10 4:2:0")]
        hevc_profile: String,
        /// H.264 profile ("Main 8-bit", "High 10-bit")
        #[arg(long, default_value = "Main 8-bit")]
        h264_profile: String,
        /// AV1 profile ("Profile 0 4:2:0 10-bit", "Profile 1 4:4:4 10-bit")
        #[arg(long, default_value = "Profile 0 4:2:0 10-bit")]
        av1_profile: String,
        /// VP9 profile ("Profile 2 4:2:0 10-bit", "Profile 3 4:4:4 10-bit")
        #[arg(long, default_value = "Profile 2 4:2:0 10-bit")]
        vp9_profile: String,
        /// Rate control: Lossless, High, Standard, Master400M, Standard150M, Custom:xxx
        #[arg(long, default_value = "Lossless")]
        rate_control: String,
        /// Lens correction mode: off, full, color-only
        #[arg(long, default_value = "full")]
        lens_correction: String,
        /// Black/white level mode: dynamic, static, 1023/64, 4095/256, 16383/1024, 65535/4096, 4095/64, 16383/64, 16383/0
        #[arg(long, default_value = "dynamic")]
        blwl: String,
        /// Override output frame rate
        #[arg(long)]
        fps: Option<f64>,
        /// Disable highlight recovery: bit-exact sensor output, no raw
        /// photosite is completed (the sensor's own clipped colour error is
        /// preserved). Default (flag absent) is the full neutral-axis
        /// reconstruction.
        #[arg(long)]
        no_highlight_recovery: bool,
        /// Diagnostic exposure shift in stops, applied as a linear gain to
        /// the mosaic after completion+lens and before demosaic (both
        /// backends identically). Negative values pull highlights out of
        /// clipping for inspection. Default 0. Not a grading control: the
        /// black pedestal scales with the gain (documented approximation).
        #[arg(long, default_value = "0.0")]
        exposure_ev: f32,
    },
}

#[derive(Parser, Debug)]
#[command(name = "mcraw-tui", about = "Cross-platform TUI for MotionCam .mcraw files")]
pub struct Cli {
    /// Path to the .mcraw file to open (backward compatibility)
    #[arg(short, long)]
    pub file: Option<String>,

    /// CLI subcommand
    #[command(subcommand)]
    pub command: Option<CliCommands>,

    /// Number of frames to load (default: all)
    #[arg(short = 'n', long)]
    pub frames: Option<usize>,

    /// Path to custom placeholder sixel file (for idle/loading animation)
    #[arg(long)]
    pub placeholder_path: Option<String>,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Output directory for extracted files
    #[arg(short, long)]
    pub output: Option<String>,
}

impl Cli {
    /// Resolve CLI arguments: subcommand -f takes precedence, falls back to top-level -f
    pub fn resolve(self) -> ResolvedCli {
        match self.command {
            Some(cmd) => ResolvedCli::Command(cmd.resolve_with_top_level(self.file)),
            None => {
                if let Some(ref file) = self.file {
                    ResolvedCli::Command(CliCommands::Open { file: Some(file.clone()) })
                } else {
                    ResolvedCli::NoFile
                }
            }
        }
    }

    /// Validate export format
    pub fn validate_export_format(format: &str) -> Result<(), String> {
        let valid = ["dng", "prores", "dnxhr", "h264", "hevc", "av1", "vp9"];
        let lower = format.to_lowercase();
        if valid.contains(&lower.as_str()) {
            Ok(())
        } else {
            Err(format!(
                "Invalid export format '{}'. Valid formats: {}",
                format,
                valid.join(", ")
            ))
        }
    }
}

impl CliCommands {
    /// Merge top-level -f into subcommand if subcommand doesn't have its own -f
    fn resolve_with_top_level(self, top_level_file: Option<String>) -> Self {
        match self {
            CliCommands::Open { file } => CliCommands::Open {
                file: file.or(top_level_file),
            },
            CliCommands::Info { file } => CliCommands::Info {
                file: file.or(top_level_file),
            },
            CliCommands::Export {
                file, format, output, color_space, transfer_function,
                prores_profile, dnxhr_profile, hevc_profile, h264_profile,
                av1_profile, vp9_profile, rate_control,
                lens_correction, blwl, fps, no_highlight_recovery,
                exposure_ev,
            } => CliCommands::Export {
                file: file.or(top_level_file),
                format, output, color_space, transfer_function,
                prores_profile, dnxhr_profile, hevc_profile, h264_profile,
                av1_profile, vp9_profile, rate_control,
                lens_correction, blwl, fps, no_highlight_recovery,
                exposure_ev,
            },
        }
    }
}

pub enum ResolvedCli {
    Command(CliCommands),
    NoFile,
}
