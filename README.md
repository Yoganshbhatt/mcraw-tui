# mcraw-tui

Cross-platform TUI for browsing, inspecting, and exporting MotionCam (`.mcraw`) files.

Reads MCRAW and MOTION format files, decodes raw Bayer data through a full color pipeline, and exports to professional video formats or CinemaDNG. All in the terminal.

## Features

- **In-browser file management**: file browser with directory navigation, favourites, hidden file toggle
- **Selective import**: import individual, selected, or all `.mcraw` files from a directory
- **Full color pipeline**: bilinear demosaic (RCD GPU with CPU fallback), white balance, color matrix, transfer function encoding
- **15 color spaces**: ACES AP1, ARRI Wide Gamut 3/4, Canon Cinema Gamut, DaVinci Wide Gamut, DCI-P3, Display P3, F-Gamut/C, Panasonic V-Gamut, Rec.2020, Rec.709, S-Gamut3/Cine, sRGB
- **15 transfer functions**: Gamma 2.4, Rec.709, Linear, PQ (ST.2084), HLG, ARRI LogC3/LogC4, Apple Log/Log 2, C-Log3, DaVinci Intermediate, F-Log2, S-Log3, V-Log, ACEScct
- **Export to**: ProRes, DNxHR, H.264, HEVC, AV1, VP9, CinemaDNG
- **Hardware encoder detection**: NVENC, AMF, QSV, VideoToolbox auto-selected when available
- **Export presets**: save/load named preset configurations
- **Batch queue**: add multiple files, render sequentially, track per-file progress
- **GPU compute demosaic** via wgpu + WGSL (graceful CPU fallback)

## Prerequisites

- **Rust** 1.74+ (edition 2021)
- **FFmpeg** 5.0+ on `PATH` — required at runtime for video encoding
- **`motioncam-decoder-rust`** — sibling directory checked out alongside this repo

### Installing FFmpeg

```bash
# macOS
brew install ffmpeg

# Ubuntu/Debian
sudo apt install ffmpeg

# Fedora
sudo dnf install ffmpeg

# Windows (Scoop)
scoop install ffmpeg

# Windows (Winget)
winget install ffmpeg
```

## Build from source

```bash
# Clone both repos
git clone https://github.com/Yoganshbhatt/mcraw-tui.git
git clone https://github.com/Yoganshbhatt/motioncam-decoder-rust.git

# Build
cd mcraw-tui
cargo build --release

# The binary is at target/release/mcraw-tui (or mcraw-tui.exe on Windows)
```

The binary is self-contained — `motioncam-decoder-rust` is statically linked. Only FFmpeg is needed at runtime.

### Platform notes

| Platform | Notes |
|---|---|
| **macOS** | Gatekeeper warning on first run: `xattr -d com.apple.quarantine mcraw-tui` |
| **Linux** | Requires glibc. No additional libraries needed beyond ffmpeg. |
| **Windows** | Run in Windows Terminal for best results. |

## CLI Usage

```bash
mcraw-tui [OPTIONS] [COMMAND]
```

### Global options

| Flag | Description |
|---|---|
| `-f, --file <FILE>` | Open a `.mcraw` file |
| `-n, --frames <N>` | Number of frames to load (default: all) |
| `-v, --verbose` | Enable verbose logging |
| `-o, --output <DIR>` | Output directory for extracted files |

### Subcommands

| Command | Description | Example |
|---|---|---|
| `open` | Open a file in the TUI (default) | `mcraw-tui open -f video.mcraw` |
| `info` | Print file metadata and exit | `mcraw-tui info -f video.mcraw` |
| `export` | Export to another format | `mcraw-tui export -f video.mcraw -F prores -o output.mov` |

### Export format flags

| Flag | Format | Codec |
|---|---|---|
| `dng` | CinemaDNG image sequence | LJ92-compressed DNG |
| `prores` | Apple ProRes | ProRes 422/444 |
| `h264` | H.264 | libx264 or hardware encoder |
| `hevc` | H.265/HEVC | libx265 or hardware encoder |

## TUI Usage

### Panel layout

```
┌─────────────────────────────────────────────────────┐
│ Header: file path, metadata, export status           │
├──────────────┬──────────────────────┬────────────────┤
│  Media Pool  │  Render Queue        │ Export Settings│
│  (imported   │  (pending/           │  (color space, │
│   .mcraw     │   rendering/         │   transfer fn, │
│   files)     │   completed/failed)  │   codec, rate) │
├──────────────┴──────────────────────┴────────────────┤
│ Browser overlay (toggle with `b`)                    │
└─────────────────────────────────────────────────────┘
```

### Workflow

1. **Open** with `mcraw-tui -f file.mcraw` — browser opens automatically if no file specified
2. **Browse** files with the file browser — navigate directories, mark favourites, toggle hidden files
3. **Import** individual files with `I` or load all `.mcraw` in current folder with `L`
4. **Configure** export settings — cycle through codec, color space, transfer function, profile, rate control
5. **Queue** items with `a` (selected) or `A` (all imported)
6. **Render** with `v` (selected) or `R` (all queued)
7. **Monitor** progress in the queue panel — cancel with `X`

### Keybindings

#### Navigation

| Key | Action |
|---|---|
| `Tab` | Cycle focus: Media Pool → Queue → Export Settings |
| `↑` / `k` | Navigate up (list, browser, favourites) |
| `↓` / `j` | Navigate down |
| `PageUp` | Fast scroll up (10 items) |
| `PageDown` | Fast scroll down |
| `Home` | Jump to start of list |
| `End` | Jump to end of list |

#### File management

| Key | Action |
|---|---|
| `Space` | Toggle selection checkbox |
| `a` | Add selected to render queue |
| `A` | Add ALL imported to render queue |
| `d` | Remove from focused panel |
| `D` | Remove ALL selected from media pool |

#### Browser

| Key | Action |
|---|---|
| `b` | Toggle browser overlay |
| `I` | Import selected `.mcraw` from browser |
| `L` | Load all `.mcraw` in current folder |
| `o` | Set export folder to current browser path |
| `f` | Toggle favourites list view |
| `F` | Toggle favourite on current path |
| `.` | Toggle hidden files |
| `Enter` | Navigate into directory / open favourite |

#### Export settings (when focused)

| Key | Action |
|---|---|
| `c` | Cycle codec family |
| `g` | Cycle gamut (color space) |
| `t` | Cycle transfer function |
| `p` | Cycle profile or begin naming preset |
| `P` | Open preset picker |
| `r` | Cycle rate control |
| `i` | Edit custom rate (when custom rate active) |

#### Actions

| Key | Action |
|---|---|
| `v` | Render selected queue items |
| `R` | Render ALL queue items |
| `x` / `X` | Cancel in-progress export OR clear completed/failed |
| `n` | Print raw metadata dump of focused file |
| `?` | Toggle help overlay |

#### General

| Key | Action |
|---|---|
| `q` | Quit |
| `Ctrl+C` | Force quit |
| `Ctrl+X` | Cancel in-progress export |
| `Esc` | Close popup → Full info → Favourites → Browser → Help → Quit |

## Export formats

The TUI supports more codecs than the CLI. Full matrix:

| Codec | Profile Options | Rate Control |
|---|---|---|
| **ProRes** | Proxy, LT, Standard, HQ, P4444, XQ4444 | Lossless, High, Standard |
| **DNxHR** | SQ, HD, HDX, HQX, P444 | Lossless, High, Standard |
| **HEVC** | Main10 420, Main10 444 | Lossless, High, Standard, Master 400M, Custom |
| **H.264** | Main 8-bit, High 10-bit | Lossless, High, Standard, Standard 150M, Custom |
| **AV1** | Profile0 420 10-bit, Profile1 444 10-bit | Lossless, High, Standard, Custom |
| **VP9** | Profile2 420 10-bit, Profile3 444 10-bit | Lossless, High, Standard, Custom |
| **DNG** | CinemaDNG image sequence (LJ92) | N/A |

Hardware encoders (NVENC, AMF, QSV, VideoToolbox) are detected at startup and preferred over software encoders when available.

## Export presets

Save and load named preset configurations. Presets are stored in the platform config directory as `presets.json`.

- Press `p` (export settings focused) to name and save the current configuration
- Press `P` to open the preset picker
- `Delete` removes a saved preset

## Color science

**Color spaces**: ACES AP1, ARRI Wide Gamut 3, ARRI Wide Gamut 4, Canon Cinema Gamut, DaVinci Wide Gamut, DCI-P3, Display P3, F-Gamut, F-Gamut C, Panasonic V-Gamut, Rec.2020, Rec.709, S-Gamut3, S-Gamut3.Cine, sRGB

**Transfer functions**: ACEScct, ARRI LogC3, ARRI LogC4, Apple Log, Apple Log 2, C-Log3, DaVinci Intermediate, F-Log2, Gamma 2.4, HLG, Linear, PQ (ST.2084), Rec.709, S-Log3, V-Log

### Pipeline order

```
Raw Bayer → Demosaic (bilinear / RCD GPU) → White balance → Color matrix (CCM) → Transfer function → RGB encoding
```

## Logging

Logs are written to the platform-specific data directory:

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/mcraw-tui/logs/` |
| Linux | `~/.local/share/mcraw-tui/logs/` |
| Windows | `%LOCALAPPDATA%/mcraw-tui/logs/` |

Logs rotate daily and are cleaned after 7 days. Set `MCRAW_TUI_LOG=<level>` to control verbosity.

## Architecture

Three-thread pipeline using bounded crossbeam channels:

```
┌──────────┐    ┌──────────────┐    ┌──────────┐
│  Loader  │───→│  Processor   │───→│  Writer  │
│ (decoder)│    │ (demosaic →  │    │ (ffmpeg  │
│          │    │  WB → CCM →  │    │  stdin)  │
│          │    │  OETF)       │    │          │
└──────────┘    └──────┬───────┘    └──────────┘
                       │ optional
                       ↓
                  ┌──────────┐
                  │  GPU     │
                  │ (wgpu)   │
                  └──────────┘
```

The processor thread can optionally dispatch RCD demosaic to a GPU compute pipeline (wgpu + WGSL) with graceful CPU fallback.

## Credits

This project builds on several open-source colour science projects. Full details in [`CREDITS.md`](CREDITS.md).

Key dependencies:
- **colour-science/colour** (BSD-3-Clause) — transfer function implementations and color space conversions
- **motioncam-decoder-rust** (Apache-2.0) — MotionCam RAW file decoding

## License

Apache-2.0
