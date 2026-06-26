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
/// Kitty protocol is checked first (modern terminals support it).
/// Falls back to sixel for known sixel terminals, then TextFallback.
pub fn detect() -> TerminalProtocol {
    let term = std::env::var("TERM").unwrap_or_default();

    // Kitty protocol — supported by Kitty, Ghostty, VS Code, Konsole, etc.
    // True 24-bit RGB, no palette limits.
    if std::env::var("KITTY_WINDOW_ID").is_ok() || std::env::var("KITTY_PID").is_ok() {
        return TerminalProtocol::Kitty;
    }
    if std::env::var("TERM_PROGRAM").as_deref().ok() == Some("Ghostty")
        || std::env::var("TERM_PROGRAM").as_deref().ok() == Some("vscode")
    {
        return TerminalProtocol::Kitty;
    }
    if term == "xterm-kitty" {
        return TerminalProtocol::Kitty;
    }

    // WezTerm — native Kitty protocol support on all platforms including
    // Windows (APC passes through ConPTY, unlike sixel's DCS which is
    // blocked). WEZTERM_EXECUTABLE is set by WezTerm in every child process.
    if std::env::var("WEZTERM_EXECUTABLE").is_ok() {
        return TerminalProtocol::Kitty;
    }

    // Sixel — foot, contour, and any terminal with "sixel" in TERM
    if term.contains("sixel") || term == "foot" || term == "contour" {
        return TerminalProtocol::Sixel;
    }

    // Unknown terminal — the encoder will fall back to sixel anyway
    // (harmless on non-sixel terminals)
    TerminalProtocol::TextFallback
}
