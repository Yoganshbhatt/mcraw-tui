# TODO

## Up Next (working on these)

1. [ ] **Save / load Export menu presets** — Persist the current export settings
   (codec, container, profile, rate, gamut, transfer, output format, export
   folder) as a named preset. Store in `presets.json` next to `favourites.json`.
   UI: `p` to save current as preset, `P` to pick from list, `Delete` in
   picker to remove. Show preset name in the Export Settings panel header.
2. [ ] **RAW editor** — In-TUI grading surface that wraps the decoded frame
   with live adjustments: exposure (stops), white balance (temp/tint),
   contrast/pivot, saturation, lift/gamma/gain (ASC CDL). Render the
   processed frame to the preview area in near real-time as the user
   scrubs. Persist per-file grade sidecar (`<file>.grade.json`) so it
   survives reloads. Keybindings live in a new `editor.rs` panel and are
   active when focus = `Editor`.
3. [ ] **Histogram / scopes** — Vector/RGB-parade + luma histogram overlay
   drawn from the current preview frame. Toggle with `H`. Three modes
   (cycle with `H`): waveform, RGB parade, vectorscope. Reuses the decoded
   pixel buffer that the RAW editor holds, so no extra decode cost.
4. [ ] **GUI popout for live grading** — Spawn a second terminal (or a
   crossterm/egui window on supported platforms) that streams the editor
   preview at a higher frame rate while the main TUI keeps the queue/panels.
   MVP: use `wezterm-cli` / `tmux split-window` / `wt.exe new-tab` heuristics
   detected in `hardware.rs`; later an optional `egui` standalone binary
   sharing the decoder crate.
5. [ ] **`install.sh` for CI artifacts** — Convenience installer so testers
   can grab a GitHub Actions build and drop the `mcraw-tui` binary on
   their `PATH` (default `~/.local/bin`) without waiting for the v1
   `cargo-dist` release. Detect OS/arch, download the matching artifact
   from the latest `dev` workflow run, install the required `motioncam-
   decoder-rust` shared lib next to it, verify `ffmpeg` is on `PATH`,
   and print a one-liner to uninstall. Idempotent and non-interactive
   (only `--prefix` and `--yes` flags).

6. [ ] **Drag-and-drop via ripdrag** — Current `Event::Paste`-based drag-drop
   is broken (inconsistent across terminals). Replace with `ripdrag` CLI
   (https://github.com/nickel-org/ripdrag), spawned as a subprocess, which
   handles OLE drag-drop on Windows, XDnD on Linux, and NSDragPasteboard on
   macOS. Expose via either a `--listen` flag or a dedicated foreground
   thread with crossterm event forwarding. Import popup (single vs all) stays
   the same.

### After these

- [ ] **Complete CLI mode** — `mcraw-tui info`, `export`, and a new
  `extract-meta` subcommand already exist; finish the remaining CLI gaps:
  recursive folder export (`export --recursive`), JSON sidecar output for
  batch metadata (`info --json`), a `grade apply` command that takes a
  sidecar CDL and renders offline, and a `verify` subcommand that checks
  an `.mcraw` file is structurally valid without decoding pixels.

## Bugs

- [ ] **`...` back-dir entry hidden under favourites bar** — When the
  favourites bar is visible and the browser list starts at row 0, the
  `..` parent-directory entry gets covered by the favourites row and
  becomes un-clickable / hard to see. Fix: reserve a row for the
  favourites bar that is *above* the list (not overlapping it), and
  offset the list render area downward by `bar_rows` so the first
  entry (`..` or the first folder) is never occluded.
- [ ] **Cannot select favourite folders with the keyboard** — Today the
  favourites bar is mouse-only / single-click only. Make `f` (or a
  dedicated `'` key) toggle the browser list into a **favourites list
  view** that replaces the current folder listing — same `↑/↓`/Enter
  keys, but the items are the favourite paths. Pressing `f` again (or
  `Esc`) returns to the normal folder view. This also fixes the
  occlusion bug above because the favourites list is rendered through
  the normal list widget, not as a header overlay.
- [ ] **Add ARRI LogC4 transfer function** — We have `ARRI Wide Gamut 4`
  in the gamut list but no matching `LogC4` transfer function. ARRI's
  LogC4 supersedes LogC3 on Alexa 35 / Alexa 265 and pairs with AWG4.
  Add `TransferFunction::ARRIlog4`:
  - OETF (scene-linear → log): two-segment with knee at `x = 1/14`
    per ARRI "LogC4 Encoding Function" (2024); expose the same
    `a`/`b`/`c`/`d`/`e` coefficients as constants.
  - EOTF inverse for round-trip in the editor path.
  - Insert in the `all()` slice **in alphabetical position** so the
    cycle order is deterministic and pleasing (also sort the existing
    gamut list alphabetically while we're here).
  - Wire up in `agx.rs` (`Transfer::ArriLogC4`) and `gpu.rs` shader
    table (`ARRIlog4 => N`).
  - Verify the pipeline doesn't break: re-run `cargo test` and a
    `headless export` with a synthetic linear ramp using AWG4 + LogC4.

## UI (medium-term)

- [ ] **Default thumbnail / splash art** — Show a Rust-themed ANSI/ASCII art
  piece as the default preview before any .mcraw file is loaded (like Durdraw
  but using Rust icons/ferris). Replace the standalone placeholder text in the
  empty-state preview area. The art should fill/frame the available panel area.

### Implementation strategy (for sixel/chafa/iterm preview engine)

The eventual preview engine should follow a layered backend architecture:

1. **Abstract `PreviewBackend` trait** with methods like:
   ```rust
   pub trait PreviewBackend {
       fn can_handle(&self) -> bool;             // is this terminal capable?
       fn render_frame(&self, pixels: &[u8], w: u16, h: u16, area: Rect) -> Span<'static>;
       fn render_thumbnail(&self, pixels: &[u8], w: u16, h: u16, area: Rect) -> Span<'static>;
   }
   ```

2. **Backend implementations** (selected at startup via capability probing):
   - **SixelBackend** — `\ePq` sequences; widely supported (xterm, mlterm,
     Windows Terminal via sixel-unsupported).
   - **KittyBackend** — Kitty's `\e_G` protocol; fastest, 24-bit.
   - **ITermBackend** — ITerm2's `\e]1337;File=` protocol.
   - **ChafaBackend** — Shells out to `chafa` for terminals with no native
     image protocol; falls back to half-block unicode art.
   - **AsciiBackend** — Pure Rust fallback using `viuer` or manual
     pixel→unicode block mapping (`▀▄█` etc.) when no external tool exists.

3. **Capability detection** (`hardware.rs` style):
   - Check `$TERM`, `$TERM_PROGRAM`, `$KITTY_WINDOW_ID`.
   - Check `COLORTERM` for 24-bit support.
   - Probe via escape-sequence queries where possible.

4. **Caching** — Store decoded+resized frame as `Arc<Vec<u8>>` per frame-index
   so scrubbing doesn't re-decode. A `FrameCache<BackendSpecific>` ring buffer.

## Future Features

### AgX Pipeline with CAT16 Chromatic Adaptation
- **Status**: Planned for future implementation
- **Purpose**: Enable professional display-referred output with CAT16 chromatic adaptation
- **Use case**: When outputting directly for display (not scene-referred for grading)
- **Notes**:
  - AgX pipeline (currently disabled) will eventually support custom output for Rec.709 and other gamuts
  - CAT16 adaptation should be optional - applied during encoding for display output, NOT for scene-referred codecs (ProRes, DNxHR, HEVC) intended for Davinci Resolve grading
  - Scene-referred data should preserve original scene colorimetry for maximum flexibility in post-production
  - CAT16 can be applied when: exporting final display output, or when user explicitly wants display-referred pipeline

## Tried but buggy (removed) — keep as historical record

### Drag-drop fallback
- **Problem**: Terminal doesn't fire `Event::Paste` in some Windows terminals (ConHost).
- **Status**: Removed.

### Log export (`l` key in Settings)
- **Problem**: Freezes UI when pressed.
- **Status**: Removed.

### Settings panel
- **Problem**: Only had log export. Now empty.
- **Status**: Removed.
