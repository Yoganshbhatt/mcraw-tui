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
        /// Export format: cdng, dng, prores, h264, hevc
        #[arg(short = 'F', long)]
        format: String,
        /// Output path or directory
        #[arg(short, long)]
        output: String,
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
        let valid = ["cdng", "dng", "prores", "h264", "hevc"];
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
            CliCommands::Export { file, format, output } => CliCommands::Export {
                file: file.or(top_level_file),
                format,
                output,
            },
        }
    }
}

pub enum ResolvedCli {
    Command(CliCommands),
    NoFile,
}
