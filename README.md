# mcraw-tui

Cross-platform TUI for browsing, inspecting, and exporting MotionCam (`.mcraw`) files.

Reads MCRAW and MOTION format files, decodes raw Bayer data through a full color pipeline (15 color spaces, 15 transfer functions, AgX tone mapping), and exports to professional video formats or CinemaDNG. All in the terminal.

## Features

- **Browse and inspect** `.mcraw` files with metadata display, frame-by-frame navigation
- **Full color pipeline**: demosaic (bilinear + RCD GPU), white balance, color matrix, transfer function encoding
- **15 color spaces**: Rec.709, Rec.2020, ACES AP1, ARRI Wide Gamut 3/4, Canon Cinema Gamut, DaVinci Wide Gamut, DCI-P3, Display P3, F-Gamut/C, Panasonic V-Gamut, S-Gamut3/Cine, sRGB
- **15 transfer functions**: Gamma 2.4, Rec.709, Linear, PQ (ST.2084), HLG (BT.2100), ARRI LogC3/LogC4, Apple Log/Log 2, C-Log3, DaVinci Intermediate, F-Log2, S-Log3, V-Log, ACEScct
- **AgX tone mapping** for spectral gamut mapping and highlight roll-off
- **Export to**: ProRes, DNxHR, H.264, HEVC, AV1, VP9, CinemaDNG
- **Hardware encoder detection**: NVENC, AMF, QSV, VideoToolbox auto-selected when available
- **GPU compute demosaic** via wgpu + WGSL (optional, graceful CPU fallback)
- **Export presets**: save/load named preset configurations
- **Batch queue**: add multiple files, render sequentially, track per-file progress
- **Per-phase pipeline timing** via `MCRAW_STATS_DUMP` env var

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

# Windows (manual)
# Download from https://ffmpeg.org/download.html and add to PATH
```

## Build from source

```bash
# Clone both repos
git clone https://github.com/Yoganshbhatt/mcraw-tui.git
git clone https://github.com/Yoganshbhatt/motioncam-decoder-rust.git

# Build
cd mcraw-tui
cargo build --release

# The binary is at target/release/mcraw-tui
```

The final binary is self-contained — `motioncam-decoder-rust` is statically linked. Only FFmpeg is needed at runtime.

### Platform-specific notes

| Platform | Notes |
|---|---|
| **Windows** | MSVC build tools required (`scoop install rustup-msvc` or Visual Studio Build Tools). Run in Windows Terminal for best results. |
| **macOS** | Tested on both Apple Silicon and Intel. Gatekeeper warning on first run: `xattr -d com.apple.quarantine mcraw-tui` |
| **Linux** | Requires glibc. No additional libraries needed beyond ffmpeg. |

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

### Export formats

| Format flag | Description | Codec |
|---|---|---|
| `dng` | CinemaDNG image sequence | LJ92-compressed DNG |
| `prores` | Apple ProRes | ProRes 422/444 |
| `h264` | H.264 | libx264 or hardware encoder |
| `hevc` | H.265/HEVC | libx265 or hardware encoder |

## TUI Usage

### Panel layout

The interface has three panels when the browser is hidden (default), four when visible:

```
┌─────────────────────────────────────────────────────┐
│ Header: file path, frame info, export status         │
├──────────────┬──────────────────────┬────────────────┤
│  Media Pool  │  Render Queue        │ Export Settings│
│  (imported   │  (pending/           │  (color space, │
│   .mcraw     │   rendering/         │   transfer fn, │
│   files)     │   completed/failed)  │   codec, rate) │
├──────────────┴──────────────────────┴────────────────┤
│ Browser overlay (toggle with `b`)                     │
└─────────────────────────────────────────────────────┘
```

### Workflow

1. **Open a file** with `mcraw-tui -f file.mcraw` or drag-and-drop a `.mcraw` onto the terminal
2. **Browse** files with `b` to open the file browser, navigate with arrow keys
3. **Import** selected files with `I` (browser) or open directly
4. **Configure** export settings in the right panel — cycle through options with `c`/`g`/`t`/`p`/`r`
5. **Queue** items with `a` (selected) or `A` (all imported)
6. **Render** with `v` (selected) or `R` (all queued)
7. **Monitor progress** in the queue panel — cancel with `X`

### Keybindings

#### Navigation

| Key | Action |
|---|---|
| `Tab` | Cycle focus: Media Pool → Queue → Export Settings |
| `↑` / `k` | Navigate up (list, browser, favourites) |
| `↓` / `j` | Navigate down |
| `←` / `h` | Previous frame |
| `→` / `l` | Next frame |
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

### Drag and drop

Drag `.mcraw` files onto the terminal window to import them. An import popup lets you choose between importing just the dropped files or all `.mcraw` files in the parent directory.

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

Hardware encoders (NVENC, AMF, QSV, VideoToolbox) are detected at startup and preferred over software encoders.

## Export presets

Save and load named preset configurations. Presets are stored in the platform config directory as `presets.json`.

- Press `p` (export settings focused) to name and save the current configuration
- Press `P` to open the preset picker
- `Delete` removes a saved preset

## Color science reference

### Color spaces

| Name | Usage |
|---|---|
| ACES AP1 | Academy Color Encoding System |
| ARRI Wide Gamut 3 | ARRI Alexa |
| ARRI Wide Gamut 4 | ARRI Alexa 35 |
| Canon Cinema Gamut | Canon Cinema cameras |
| DaVinci Wide Gamut | Blackmagic Design |
| DCI-P3 | Digital cinema projection |
| Display P3 | Apple displays |
| F-Gamut / F-Gamut C | Fujifilm |
| Panasonic V-Gamut | Panasonic Varicam |
| Rec.2020 | UHDTV / BT.2020 |
| Rec.709 | HDTV / BT.709 |
| S-Gamut3 / S-Gamut3.Cine | Sony |
| sRGB | Web / SDR displays |

### Transfer functions

| Name | Type |
|---|---|
| Linear | Scene-linear |
| Gamma 2.4 | SDR display |
| Rec.709 | SDR broadcast |
| PQ (ST.2084) | HDR10 |
| HLG (BT.2100) | Hybrid Log-Gamma |
| ARRI LogC3 / LogC4 | ARRI log |
| Apple Log / Apple Log 2 | Apple log |
| C-Log3 | Canon log |
| DaVinci Intermediate | Blackmagic intermediate |
| F-Log2 | Fujifilm log |
| S-Log3 | Sony log |
| V-Log | Panasonic log |
| ACEScct | ACES log |

### Pipeline order

```
Raw Bayer → Demosaic → White balance → Color matrix (CCM) →
Transfer function → Optional AgX tone mapping → RGB encoding
```

## Logging

Logs are written to the platform-specific data directory:

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/mcraw-tui/logs/` |
| Linux | `~/.local/share/mcraw-tui/logs/` |
| Windows | `%LOCALAPPDATA%/mcraw-tui/logs/` |

Logs rotate daily and are cleaned after 7 days. Set `MCRAW_TUI_LOG=<level>` to control verbosity.

## Performance statistics

Set `MCRAW_STATS_DUMP=stats.json` when running export to write per-phase timing data:

```bash
MCRAW_STATS_DUMP=export-timing.json mcraw-tui export -f video.mcraw -F prores -o output.mov
```

Phase timings recorded: setup, decode, demosaic, normalize, white balance, CCM, OETF, pack, GPU, encode push, finalize.

## Architecture

```
┌──────────┐    ┌──────────────┐    ┌──────────┐
│  Loader  │───→│  Processor   │───→│  Writer  │
│ (decoder)│    │ (demosaic →  │    │ (ffmpeg  │
│          │    │  WB → CCM →  │    │  stdin)  │
│          │    │  OETF)       │    │          │
└──────────┘    └──────────────┘    └──────────┘
                    │ optional
                    ↓
               ┌──────────┐
               │  GPU     │
               │ (wgpu)   │
               └──────────┘
```

Three-thread pipeline using bounded crossbeam channels. Each frame slot is pre-allocated (`PIPELINE_DEPTH = 3`). The processor thread can optionally dispatch RCD demosaic to a GPU compute pipeline (wgpu + WGSL) with graceful CPU fallback.

## Credits

This project builds on several open-source colour science projects. Full details in [`CREDITS.md`](CREDITS.md).

Key dependencies:
- **colour-science/colour** (BSD-3-Clause) — transfer function implementations and color space conversions
- **AgX / Troy Sobotka** (MIT) — AgX tone mapping pipeline
- **motioncam-decoder-rust** (Apache-2.0) — MotionCam RAW file decoding

## License

Apache-2.0
