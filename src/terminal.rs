use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TerminalProtocol {
    Sixel,
    Kitty,
    TextFallback,
}

static PROTOCOL: OnceLock<TerminalProtocol> = OnceLock::new();

pub fn init(p: TerminalProtocol) {
    _ = PROTOCOL.set(p);
}

pub fn protocol() -> TerminalProtocol {
    *PROTOCOL.get().unwrap_or(&TerminalProtocol::TextFallback)
}

pub fn protocol_name(p: TerminalProtocol) -> &'static str {
    match p {
        TerminalProtocol::Sixel => "sixel",
        TerminalProtocol::Kitty => "kitty",
        TerminalProtocol::TextFallback => "text-fallback",
    }
}

/// Detect the terminal image protocol from environment variables.
///
/// Kitty protocol (24-bit RGBA, no palette limits) is checked first and
/// preferred. Sixel (palette-based, DEC-originated) is the fallback for
/// terminals that don't support Kitty. Both encoders exist and work.
///
/// As of mid-2026 these are the terminals per platform:
///
/// | Platform  | Kitty                          | Sixel                     |
/// |-----------|--------------------------------|---------------------------|
/// | Linux     | kitty, Ghostty, Konsole,       | foot, contour, mlterm,    |
/// |           | WezTerm (via APC passthrough)  | WezTerm, xterm+sixel      |
/// | macOS     | kitty, Ghostty, WezTerm,       | WezTerm, Terminal.app,    |
/// |           | iTerm2 3.6+, Terminal.app,     |                           |
/// |           | Warp                           |                           |
/// | Windows   | WezTerm, Windows Terminal 1.22+| WT 1.22+, mintty, ConEmu  |
pub fn detect() -> TerminalProtocol {
    // Manual override: MCRAW_FORCE_PROTOCOL=kitty | sixel | text
    // Use when env-var auto-detection misses your terminal.
    if let Ok(force) = std::env::var("MCRAW_FORCE_PROTOCOL") {
        match force.to_lowercase().as_str() {
            "kitty" => return TerminalProtocol::Kitty,
            "sixel" => return TerminalProtocol::Sixel,
            "text" => return TerminalProtocol::TextFallback,
            _ => {}  // unrecognised value → fall through to auto-detect
        }
    }

    let term = std::env::var("TERM").unwrap_or_default();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();

    // ── Kitty Graphics Protocol ─────────────────────────────────────
    // Checked first — highest quality: 24-bit RGBA, no palette limits.

    // Kitty / Kitten
    if std::env::var("KITTY_WINDOW_ID").is_ok()
        || std::env::var("KITTY_PID").is_ok()
        || term == "xterm-kitty"
    {
        return TerminalProtocol::Kitty;
    }

    // Ghostty
    if term_program == "Ghostty" {
        return TerminalProtocol::Kitty;
    }

    // VS Code integrated terminal (supported via xterm.js)
    if term_program == "vscode" {
        return TerminalProtocol::Kitty;
    }

    // WezTerm — supports both protocols on all platforms; prefer Kitty
    if std::env::var("WEZTERM_EXECUTABLE").is_ok() {
        return TerminalProtocol::Kitty;
    }

    // iTerm2 3.6+ (macOS) — adopted Kitty graphics protocol
    if term_program == "iTerm.app" {
        return TerminalProtocol::Kitty;
    }

    // Konsole (KDE) — supports both since 22.04; prefer Kitty
    if std::env::var("KONSOLE_VERSION").is_ok() {
        return TerminalProtocol::Kitty;
    }

    // Warp — sets WARP_IS_LOCAL_SHELL_SESSION in local shells
    if term_program == "WarpTerminal" || std::env::var("WARP_IS_LOCAL_SHELL_SESSION").is_ok() {
        return TerminalProtocol::Kitty;
    }

    // Rio — GPU-accelerated terminal with native Kitty support
    if term_program == "Rio" {
        return TerminalProtocol::Kitty;
    }

    // Terminal.app (macOS) — native Kitty graphics support (per terminfo.dev)
    if term_program == "Apple_Terminal" {
        return TerminalProtocol::Kitty;
    }

    // Tabby (formerly Terminus)
    if term_program == "Tabby" {
        return TerminalProtocol::Kitty;
    }

    // ── Sixel Graphics Protocol ─────────────────────────────────────
    // Legacy DEC-originated protocol, palette-based (256-colour).

    // TERM containing "sixel" — generic sixel-capable terminal
    if term.contains("sixel") {
        return TerminalProtocol::Sixel;
    }

    // foot (Wayland) — native sixel support
    if term == "foot" || term.starts_with("foot-") {
        return TerminalProtocol::Sixel;
    }

    // contour — sixel-native terminal emulator
    if term == "contour" {
        return TerminalProtocol::Sixel;
    }

    // mlterm — lightweight multi-lingual terminal with sixel
    if term == "mlterm" {
        return TerminalProtocol::Sixel;
    }

    // mintty (MSYS2 / Cygwin / Git Bash on Windows) — sixel since 3.7.
    // On Windows, TERM=xterm-256color + MSYSTEM/MINGW_PREFIX is mintty.
    if (std::env::var("MSYSTEM").is_ok() || std::env::var("MINGW_PREFIX").is_ok())
        && (term.starts_with("xterm") || term == "cygwin")
    {
        return TerminalProtocol::Sixel;
    }

    // Windows Terminal 1.22+ — native sixel support through ConPTY.
    // WT_SESSION is the canonical detection method on Windows
    // (TERM_PROGRAM is not set by WT). Kitty protocol support is not
    // confirmed on WT, so we stick with sixel which is known to work.
    if std::env::var("WT_SESSION").is_ok() {
        return TerminalProtocol::Sixel;
    }

    // ConEmu / ConsoleZ on Windows — supports sixel via DCS passthrough
    if std::env::var("ConEmuPID").is_ok() || std::env::var("ConEmuHWND").is_ok() {
        return TerminalProtocol::Sixel;
    }

    // ── Fallback ────────────────────────────────────────────────────
    TerminalProtocol::TextFallback
}
