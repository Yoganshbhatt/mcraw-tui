use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use std::time::Duration;

use crate::app::{App, ExportFocus, FocusTarget, ImportPopupState, QueueStatus};
use crate::export::CodecFamily;

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

struct Palette;
impl Palette {
    const HEADER_BG: Color = Color::Rgb(25, 45, 85);
    const HEADER_FG: Color = Color::White;
    const BORDER: Color = Color::Rgb(70, 100, 140);
    const BORDER_FOCUSED: Color = Color::Rgb(230, 190, 60);
    const LABEL: Color = Color::Rgb(150, 150, 170);
    const VALUE: Color = Color::White;
    const FOCUSED: Color = Color::Rgb(230, 190, 60);
    const CHECKED: Color = Color::Rgb(80, 210, 120);
    const UNCHECKED: Color = Color::Rgb(90, 90, 110);
    const HIGHLIGHT_BG: Color = Color::Rgb(45, 55, 90);
    const HIGHLIGHT_FOCUSED_BG: Color = Color::Rgb(60, 70, 120);
    const QUEUE_WAITING: Color = Color::Rgb(150, 150, 170);
    const QUEUE_RENDERING: Color = Color::Rgb(230, 190, 60);
    const QUEUE_COMPLETED: Color = Color::Rgb(80, 210, 120);
    const QUEUE_FAILED: Color = Color::Rgb(220, 80, 80);
    const BUTTON_BG: Color = Color::Rgb(50, 90, 170);
    const BUTTON_FG: Color = Color::White;
    const STATUS_KEY: Color = Color::Rgb(110, 190, 230);
    const POPUP_TITLE: Color = Color::Rgb(230, 190, 60);
    const POPUP_BORDER: Color = Color::Rgb(80, 120, 160);
    const SUCCESS: Color = Color::Rgb(80, 210, 120);
    const BROWSER_DIR: Color = Color::Rgb(110, 190, 230);
    const BROWSER_MCRAW: Color = Color::Rgb(210, 210, 230);
    const BROWSER_OTHER: Color = Color::Rgb(80, 80, 100);
    const HW_CODEC: Color = Color::Rgb(80, 210, 120);
    const SW_CODEC: Color = Color::Rgb(220, 140, 60);
    const PROGRESS_BAR_BG: Color = Color::Rgb(40, 40, 60);
    const PROGRESS_BAR_FG: Color = Color::Rgb(80, 210, 120);
    const IMPORT_PROMPT: Color = Color::Rgb(130, 160, 200);
    const PANEL_BG: Color = Color::Rgb(20, 25, 45);
}

// ---------------------------------------------------------------------------
// Click region system
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ClickRegion {
    pub area: Rect,
    pub action: ClickAction,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClickAction {
    ToggleBrowser,
    ToggleFileSelection(usize),
    ToggleQueueSelection(usize),
    SelectMediaPoolItem(usize),
    SelectQueueItem(usize),
    FocusMediaPool,
    FocusQueue,
    FocusExport,
    FocusPreview,
    AddSelectedToQueue,
    AddAllToQueue,
    RenderSelected,
    RenderAll,
    ClearQueue,
    CycleCodec,
    CycleGamut,
    CycleTransfer,
    CycleProfile,
    CycleRate,
    ImportOption1,
    ImportOption2,
    ClosePopup,
    ToggleHelp,
    BrowserNavigate(usize),
    BrowserSelectAndEnter(usize),
    BrowserEnter,
    BrowserGoUp,
    RemoveSelectedFromMediaPool,
    ToggleBrowserSelection(usize),
    FavouriteNavigate(usize),
    OpenPresetPicker,
}

// ---------------------------------------------------------------------------
// Render entry point
// ---------------------------------------------------------------------------

pub fn render(frame: &mut Frame, app: &App, regions: &mut Vec<ClickRegion>) {
    let size = frame.area();
    frame.render_widget(Clear, size);

    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(2),
        ])
        .split(size);

    render_header(frame, vert[0], app, regions);

    if app.imported_files.is_empty() && !app.show_browser {
        render_empty_state(frame, vert[1], app, regions);
    } else if app.imported_files.is_empty() {
        // Show a minimal body so browser overlay has something to render over
        let body_block = ratatui::widgets::Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Palette::BORDER));
        frame.render_widget(body_block, vert[1]);
    } else if app.show_culling {
        render_culling_screen(frame, vert[1], app, regions);
    } else {
        render_body(frame, vert[1], app, regions);
    }

    render_status(frame, app, vert[2], regions);

    // Overlays render LAST so they appear on top
    if app.show_browser {
        render_browser_overlay(frame, size, app, regions);
    }
    if app.import_popup != ImportPopupState::Hidden {
        render_import_popup(frame, size, app, regions);
    }
    if app.show_full_info {
        render_full_info_overlay(frame, size, app);
    }
    if app.show_help {
        render_help_overlay(frame, app, size);
    }
    if app.preset_picker.open {
        render_preset_picker(frame, size, app);
    }
    if app.preset_naming.is_some() {
        render_preset_naming(frame, size, app);
    }

    // Drop preview overlay - shows briefly after files are dropped
    if let Some(ref preview) = app.drop_preview {
        if preview.start_time.elapsed() < Duration::from_secs(2) {
            render_drop_preview(frame, size, preview);
        }
    }
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn render_header(frame: &mut Frame, area: Rect, app: &App, regions: &mut Vec<ClickRegion>) {
    let mut spans = vec![
        Span::styled(" mcraw-tui ", Style::default().fg(Palette::HEADER_FG).bg(Palette::HEADER_BG).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
    ];
    if let Some(ref path) = app.file_path {
        let name = path.split(std::path::MAIN_SEPARATOR).last().unwrap_or(path);
        spans.push(Span::styled(name, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled(format!("{} imported", app.imported_files.len()), Style::default().fg(Color::White)));
    spans.push(Span::raw("  |  "));
    spans.push(Span::styled(format!("Queue: {}", app.queue.len()), Style::default().fg(Color::White)));
    if app.is_exporting {
        spans.push(Span::raw("  |  "));
        spans.push(Span::styled(format!("[{:.0}%]", app.export_progress), Style::default().fg(Palette::SUCCESS).add_modifier(Modifier::BOLD)));
    }

    let toggle_label = if app.show_browser { "[Hide] Browser" } else { "[Show] Browser" };
    let toggle_style = Style::default().fg(Palette::STATUS_KEY).add_modifier(Modifier::BOLD);
    spans.push(Span::raw("  "));
    spans.push(Span::styled(toggle_label, toggle_style));

    let toggle_area = Rect {
        x: area.x + area.width.saturating_sub(18),
        y: area.y,
        width: 18,
        height: area.height,
    };
    regions.push(ClickRegion { area: toggle_area, action: ClickAction::ToggleBrowser });

    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Palette::BORDER)),
        ),
        area,
    );
}

// ---------------------------------------------------------------------------
// Empty state (no files imported)
// ---------------------------------------------------------------------------

fn render_empty_state(frame: &mut Frame, area: Rect, app: &App, regions: &mut Vec<ClickRegion>) {
    let lines = vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "  Import .mcraw files to get started",
            Style::default().fg(Palette::IMPORT_PROMPT).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Press [b] to toggle file browser",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Or drag & drop .mcraw files onto this window",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "  [b] Toggle Browser    [?] Help",
            Style::default().fg(Palette::STATUS_KEY).add_modifier(Modifier::BOLD),
        )),
    ];

    let panel = Paragraph::new(lines)
        .alignment(ratatui::layout::Alignment::Center)
        .block(
            Block::default()
                .title(" Welcome ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Palette::BORDER)),
        );
    frame.render_widget(panel, area);
}

// ---------------------------------------------------------------------------
// Body layout - 2x2 grid
// ---------------------------------------------------------------------------

fn render_body(frame: &mut Frame, area: Rect, app: &App, regions: &mut Vec<ClickRegion>) {
    // 2x2 grid: Media Pool | Preview/Progress
    //           Export       | Render Queue
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(area);

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(35),
            Constraint::Percentage(65),
        ])
        .split(vert[0]);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(35),
            Constraint::Percentage(65),
        ])
        .split(vert[1]);

    render_media_pool(frame, app, top[0], regions);
    render_preview_or_progress(frame, app, top[1], regions);
    render_export_settings(frame, app, bottom[0], regions);
    render_queue_panel(frame, app, bottom[1], regions);
}

// ---------------------------------------------------------------------------
// Culling screen
// ---------------------------------------------------------------------------

fn render_culling_screen(frame: &mut Frame, area: Rect, app: &App, regions: &mut Vec<ClickRegion>) {
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(70),
        ])
        .split(area);

    // Left panel: file list with checkboxes
    let left_inner = horiz[0].height.saturating_sub(2) as usize;
    let is_left_focused = app.focus_target == FocusTarget::MediaPool;
    let left_border = if is_left_focused { Palette::BORDER_FOCUSED } else { Palette::BORDER };

    let items: Vec<ListItem> = app.imported_files.iter().enumerate().map(|(_i, f)| {
        let name = f.path.split(std::path::MAIN_SEPARATOR).last().unwrap_or(&f.path);
        let checkbox = if f.selected {
            Span::styled("[*] ", Style::default().fg(Palette::CHECKED).add_modifier(Modifier::BOLD))
        } else {
            Span::styled("[ ] ", Style::default().fg(Palette::UNCHECKED))
        };
        let content = Line::from(vec![
            checkbox,
            Span::styled(name, Style::default().fg(Color::White)),
            Span::raw("  "),
            Span::styled(format!("{}x{}", f.info.width, f.info.height), Style::default().fg(Color::Cyan)),
        ]);
        ListItem::new(content)
    }).collect();

    let list = List::new(items)
        .block(Block::default().title(format!(" Culling ({}) ", app.imported_files.len())).borders(Borders::ALL).border_style(Style::default().fg(left_border)))
        .highlight_style(if is_left_focused {
            Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD).bg(Palette::HIGHLIGHT_FOCUSED_BG)
        } else {
            Style::default().fg(Color::White).bg(Palette::HIGHLIGHT_BG)
        })
        .highlight_symbol("> ");
    let mut state = ListState::default();
    state.select(Some(app.media_pool_index));
    frame.render_stateful_widget(list, horiz[0], &mut state);

    // Right panel: large preview / info for the selected file
    let right_border = if app.focus_target == FocusTarget::Preview { Palette::BORDER_FOCUSED } else { Palette::BORDER };
    if let Some(info) = app.focused_file_info().or(app.file_info.as_ref()) {
        let name = info.path.split(std::path::MAIN_SEPARATOR).last().unwrap_or(&info.path);
        let text = vec![
            Line::from(Span::styled(format!(" {}", name), Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from(vec![Span::styled("  Resolution: ", Style::default().fg(Palette::LABEL)), Span::styled(format!("{} x {}", info.width, info.height), Style::default().fg(Palette::VALUE))]),
            Line::from(vec![Span::styled("  Frames:     ", Style::default().fg(Palette::LABEL)), Span::styled(format!("{}", info.frame_count), Style::default().fg(Palette::VALUE))]),
            Line::from(vec![Span::styled("  FPS:        ", Style::default().fg(Palette::LABEL)), Span::styled(format!("{:.1}", info.fps), Style::default().fg(Palette::VALUE))]),
            Line::from(vec![Span::styled("  Camera:     ", Style::default().fg(Palette::LABEL)), Span::styled(info.camera_metadata.camera_model.as_deref().unwrap_or("MotionCam"), Style::default().fg(Palette::VALUE))]),
            Line::from(""),
            Line::from(Span::styled("                ╱|_______ ", Style::default().fg(Color::Yellow))),
            Line::from(Span::styled("               (˶❛_❛˵)  /  ", Style::default().fg(Color::Yellow))),
            Line::from(Span::styled("                ^^     ^^   ", Style::default().fg(Color::Yellow))),
            Line::from(""),
            Line::from(Span::styled("  Space  Toggle  |  a  Add to Queue  |  C  Exit culling", Style::default().fg(Color::DarkGray))),
        ];
        let panel = Paragraph::new(text)
            .block(Block::default().title(" Preview ").borders(Borders::ALL).border_style(Style::default().fg(right_border)))
            .wrap(Wrap { trim: false });
        frame.render_widget(panel, horiz[1]);
    } else {
        let text = vec![
            Line::from(Span::styled(" PREVIEW", Style::default().fg(Palette::LABEL).add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from(Span::styled("  No file selected", Style::default().fg(Color::DarkGray))),
        ];
        let panel = Paragraph::new(text)
            .block(Block::default().title(" Preview ").borders(Borders::ALL).border_style(Style::default().fg(right_border)));
        frame.render_widget(panel, horiz[1]);
    }
}

// ---------------------------------------------------------------------------
// Browser overlay
// ---------------------------------------------------------------------------

fn render_browser_overlay(frame: &mut Frame, area: Rect, app: &App, regions: &mut Vec<ClickRegion>) {
    let browser_area = Rect {
        x: area.x,
        y: area.y + 3,
        width: area.width / 3,
        height: area.height.saturating_sub(5),
    };

    frame.render_widget(Clear, browser_area);

    // Inner dimensions once the border is accounted for.
    let inner_h = browser_area.height.saturating_sub(2);
    let has_room_for_buttons = inner_h >= 3;

    // We now render a vertical stack INSIDE the bordered block:
    //   [ favourites bar? ] [ list area ] [ button row? ]
    // The favourites bar is given its own row so it can never occlude the
    // `..` entry or any other list item (previously it was rendered after
    // the List widget as an overlay, which hid row 0).
    let show_fav_bar = app.show_favourites_bar
        && !app.browsing_favourites
        && !app.favourite_folders.is_empty();
    let bar_rows: u16 = if show_fav_bar { 1 } else { 0 };
    let button_rows: u16 = if has_room_for_buttons { 1 } else { 0 };

    let inner_x = browser_area.x + 1;
    let inner_w = browser_area.width.saturating_sub(2);
    let inner_y = browser_area.y + 1;

    let bar_area = Rect {
        x: inner_x,
        y: inner_y,
        width: inner_w,
        height: bar_rows,
    };
    let list_y = inner_y + bar_rows;
    let list_h = inner_h.saturating_sub(bar_rows + button_rows);
    let list_area = Rect {
        x: inner_x,
        y: list_y,
        width: inner_w,
        height: list_h,
    };
    let button_y = inner_y + inner_h.saturating_sub(button_rows);
    let button_area = Rect {
        x: inner_x + 1,
        y: button_y,
        width: inner_w.saturating_sub(2),
        height: button_rows,
    };

    // Title reflects the current mode (folder list vs favourites list).
    let path_display = app.browser.current_path_display();
    let title = if app.browsing_favourites {
        format!(" Favourites (Esc/f to return) ")
    } else {
        format!(" Browse: {} ", path_display)
    };

    // 1) Pinned favourites bar (drawn in its own row, not as an overlay).
    if show_fav_bar {
        let mut x = bar_area.x + 1;
        let star_style = Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("★", star_style))),
            Rect { x: bar_area.x, y: bar_area.y, width: 1, height: 1 },
        );
        for (i, f) in app.favourite_folders.iter().enumerate() {
            if x >= bar_area.x + bar_area.width.saturating_sub(3) {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled("…", Style::default().fg(Color::DarkGray)))),
                    Rect { x, y: bar_area.y, width: 1, height: 1 },
                );
                break;
            }
            let disp = f.file_name().map(|n| n.to_string_lossy()).unwrap_or_else(|| f.to_string_lossy());
            let text = format!(" {} ", disp);
            let item_style = Style::default().fg(Color::Cyan).bg(Palette::HIGHLIGHT_BG);
            let item_area = Rect { x, y: bar_area.y, width: text.len() as u16, height: 1 };
            frame.render_widget(Paragraph::new(Line::from(Span::styled(&text, item_style))), item_area);
            regions.push(ClickRegion { area: item_area, action: ClickAction::FavouriteNavigate(i) });
            x = x.saturating_add(text.len() as u16 + 1);
        }
    }

    // 2) List area: either the favourites list (full replace) or the
    //    normal browser entries.
    if app.browsing_favourites {
        let items: Vec<ListItem> = app
            .favourite_folders
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let disp = f
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| f.to_string_lossy().into_owned());
                let full = f.display().to_string();
                let content = vec![
                    Span::styled("★ ", Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{:<24}", disp), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::styled(full, Style::default().fg(Palette::LABEL)),
                ];
                let _ = i;
                ListItem::new(Line::from(content))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Palette::BORDER_FOCUSED))
                    .title(title),
            )
            .highlight_style(
                Style::default()
                    .fg(Palette::FOCUSED)
                    .add_modifier(Modifier::BOLD)
                    .bg(Palette::HIGHLIGHT_BG),
            )
            .highlight_symbol("> ");

        let mut state = ListState::default()
            .with_offset(app.favourites_scroll_offset.get());
        state.select(Some(app.favourites_scroll_offset.get()));
        frame.render_stateful_widget(list, list_area, &mut state);
        // Keep the offset in sync (handles clamping done by the widget).
        if let Some(off) = state.offset().into() {
            app.favourites_scroll_offset.set(off);
        }
    } else {
        let items: Vec<ListItem> = app
            .browser
            .entries
            .iter()
            .enumerate()
            .map(|(_i, entry)| {
                let is_mcraw = entry.name.to_lowercase().ends_with(".mcraw");
                let checkbox = if is_mcraw {
                    if entry.selected {
                        Span::styled("[*] ", Style::default().fg(Palette::CHECKED).add_modifier(Modifier::BOLD))
                    } else {
                        Span::styled("[ ] ", Style::default().fg(Palette::UNCHECKED))
                    }
                } else {
                    Span::styled("    ", Style::default())
                };
                let name_style = if entry.is_dir {
                    Style::default().fg(Palette::BROWSER_DIR)
                } else if is_mcraw {
                    Style::default().fg(Palette::BROWSER_MCRAW)
                } else {
                    Style::default().fg(Palette::BROWSER_OTHER)
                };
                let mut content = vec![
                    checkbox,
                    Span::styled(&entry.name, name_style),
                ];
                if let Some(ref info) = entry.file_info {
                    content.push(Span::raw("  "));
                    content.push(Span::styled(
                        format!("{}x{}", info.width, info.height),
                        Style::default().fg(Palette::SUCCESS),
                    ));
                }
                ListItem::new(Line::from(content))
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Palette::BORDER_FOCUSED))
                    .title(title),
            )
            .highlight_style(
                Style::default()
                    .fg(Palette::FOCUSED)
                    .add_modifier(Modifier::BOLD)
                    .bg(Palette::HIGHLIGHT_BG),
            )
            .highlight_symbol("> ");

        let mut state = ListState::default()
            .with_offset(app.browser_scroll_offset.get());
        state.select(Some(app.browser.selected_index));
        frame.render_stateful_widget(list, list_area, &mut state);
        app.browser_scroll_offset.set(state.offset());
    }

    // 3) Button row (bottom of inner area).
    if has_room_for_buttons {
        let import_btn = Rect { x: button_area.x, y: button_area.y, width: 16, height: 1 };
        regions.push(ClickRegion { area: import_btn, action: ClickAction::ImportOption1 });
        let all_btn = Rect { x: button_area.x + 17, y: button_area.y, width: 10, height: 1 };
        regions.push(ClickRegion { area: all_btn, action: ClickAction::ImportOption2 });
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" [I] Import Sel ", Style::default().fg(Palette::BUTTON_FG).bg(Palette::BUTTON_BG).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(" [L] All ", Style::default().fg(Palette::BUTTON_FG).bg(Palette::BUTTON_BG).add_modifier(Modifier::BOLD)),
            ])),
            button_area,
        );
    }

    // 4) Click regions for list items (only meaningful for the normal
    //    browser list — the favourites list is keyboard-driven).
    if !app.browsing_favourites {
        // The List widget draws its own border inside `list_area`, so its
        // items live at `list_area.y + 1` onward. The last inner row
        // (`list_area.height - 2`) is the bottom border — skip it.
        let visible_rows = list_area.height.saturating_sub(2) as usize;
        let visible_start = app.browser_scroll_offset.get();
        for i in 0..visible_rows {
            let entry_index = visible_start + i;
            if entry_index >= app.browser.entries.len() {
                break;
            }
            let is_mcraw = app.browser.entries[entry_index]
                .name
                .to_lowercase()
                .ends_with(".mcraw");

            if is_mcraw {
                let cb_area = Rect {
                    x: list_area.x + 1,
                    y: list_area.y + 1 + i as u16,
                    width: 4,
                    height: 1,
                };
                regions.push(ClickRegion {
                    area: cb_area,
                    action: ClickAction::ToggleBrowserSelection(entry_index),
                });
            }

            let row_area = Rect {
                x: list_area.x + 5,
                y: list_area.y + 1 + i as u16,
                width: list_area.width.saturating_sub(6),
                height: 1,
            };
            let action = if is_mcraw {
                ClickAction::BrowserSelectAndEnter(entry_index)
            } else {
                ClickAction::BrowserNavigate(entry_index)
            };
            regions.push(ClickRegion { area: row_area, action });
        }
    }
}

// ---------------------------------------------------------------------------
// Media pool
// ---------------------------------------------------------------------------

fn render_media_pool(frame: &mut Frame, app: &App, area: Rect, regions: &mut Vec<ClickRegion>) {
    let is_focused = app.focus_target == FocusTarget::MediaPool;
    let border_color = if is_focused { Palette::BORDER_FOCUSED } else { Palette::BORDER };
    let inner_h = area.height.saturating_sub(2) as usize;

    // Panel-wide click region to focus media pool
    regions.push(ClickRegion { area, action: ClickAction::FocusMediaPool });

    let items: Vec<ListItem> = app.imported_files.iter().enumerate().map(|(_i, f)| {
        let name = f.path.split(std::path::MAIN_SEPARATOR).last().unwrap_or(&f.path);
        let checkbox = if f.selected {
            Span::styled("[*] ", Style::default().fg(Palette::CHECKED).add_modifier(Modifier::BOLD))
        } else {
            Span::styled("[ ] ", Style::default().fg(Palette::UNCHECKED))
        };
        let res = format!("{}x{}", f.info.width, f.info.height);
        let fps = format!("{:.0}fps", f.info.fps);
        let frames = format!("{}frm", f.info.frame_count);
        let content = Line::from(vec![
            checkbox,
            Span::styled(name, Style::default().fg(Color::White)),
            Span::raw("  "),
            Span::styled(res, Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled(fps, Style::default().fg(Palette::SUCCESS)),
            Span::raw("  "),
            Span::styled(frames, Style::default().fg(Color::Gray)),
        ]);
        ListItem::new(content)
    }).collect();

    if items.is_empty() {
        let placeholder = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("  No files imported", Style::default().fg(Color::DarkGray))),
        ]).block(
            Block::default()
                .title(" Media Pool ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        );
        frame.render_widget(placeholder, area);
    } else {
        // Place buttons at the bottom of the panel, accounting for scroll
        let has_room_for_buttons = inner_h >= 3;
        let visible_items = if has_room_for_buttons { inner_h - 1 } else { inner_h };

        let list = List::new(items)
            .block(
                Block::default()
                    .title(format!(" Media Pool ({}) ", app.imported_files.len()))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color)),
            )
            .highlight_style(
                if is_focused {
                    Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD).bg(Palette::HIGHLIGHT_FOCUSED_BG)
                } else {
                    Style::default().fg(Color::White).bg(Palette::HIGHLIGHT_BG)
                },
            )
            .highlight_symbol("> ");

        let mut state = ListState::default();
        state.select(Some(app.media_pool_index));
        frame.render_stateful_widget(list, area, &mut state);

        // Render buttons at the bottom row if there's room
        if has_room_for_buttons {
            let btn_y = area.y + area.height.saturating_sub(2);
            let btn_row = Rect {
                x: area.x + 2,
                y: btn_y,
                width: area.width.saturating_sub(4),
                height: 1,
            };

            let add_btn = Rect { x: btn_row.x, y: btn_row.y, width: 12, height: 1 };
            regions.push(ClickRegion { area: add_btn, action: ClickAction::AddSelectedToQueue });

            let add_all_btn = Rect { x: btn_row.x + 13, y: btn_row.y, width: 10, height: 1 };
            regions.push(ClickRegion { area: add_all_btn, action: ClickAction::AddAllToQueue });

            let del_btn = Rect { x: btn_row.x + 24, y: btn_row.y, width: 10, height: 1 };
            regions.push(ClickRegion { area: del_btn, action: ClickAction::RemoveSelectedFromMediaPool });

            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" [a] Add ", Style::default().fg(Palette::BUTTON_FG).bg(Palette::BUTTON_BG).add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::styled(" [A] All ", Style::default().fg(Palette::BUTTON_FG).bg(Palette::BUTTON_BG).add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::styled(" [D] Del ", Style::default().fg(Palette::BUTTON_FG).bg(Palette::BUTTON_BG).add_modifier(Modifier::BOLD)),
                ])),
                btn_row,
            );
        }

        // Calculate scroll offset to match List widget behavior
        let visible_start = if app.media_pool_index >= visible_items {
            app.media_pool_index - visible_items + 1
        } else {
            0
        };

        for i in 0..visible_items.min(app.imported_files.len()) {
            let entry_index = visible_start + i;
            if entry_index >= app.imported_files.len() {
                break;
            }
            let row_y = area.y + 1 + i as u16;
            let cb_area = Rect { x: area.x + 2, y: row_y, width: 4, height: 1 };
            regions.push(ClickRegion { area: cb_area, action: ClickAction::ToggleFileSelection(entry_index) });
            let row_area = Rect { x: area.x + 6, y: row_y, width: area.width.saturating_sub(8), height: 1 };
            regions.push(ClickRegion { area: row_area, action: ClickAction::SelectMediaPoolItem(entry_index) });
        }
    }
}

// ---------------------------------------------------------------------------
// Preview or render progress panel
// ---------------------------------------------------------------------------

fn render_preview_or_progress(frame: &mut Frame, app: &App, area: Rect, _regions: &mut Vec<ClickRegion>) {
    let is_focused = app.focus_target == FocusTarget::Preview;
    let border_color = if is_focused { Palette::BORDER_FOCUSED } else { Palette::BORDER };

    if app.is_exporting {
        render_render_progress(frame, app, area, border_color);
    } else if app.last_export_summary.is_some() {
        render_export_summary(frame, app, area, border_color);
    } else {
        render_preview_panel(frame, app, area, border_color);
    }
}

/// Post-render summary panel. Shown after an export finishes (success,
/// failure, or cancellation) until the user starts another export. Mirrors
/// the "render complete" panel in DaVinci Resolve — sticky settings + timing.
fn render_export_summary(frame: &mut Frame, app: &App, area: Rect, border_color: Color) {
    let summary = match app.last_export_summary.as_ref() {
        Some(s) => s,
        None => return,
    };

    let elapsed_secs = summary.elapsed.as_secs();
    let mins = elapsed_secs / 60;
    let secs = elapsed_secs % 60;
    let elapsed_str = if mins > 0 {
        format!("{}m {:02}s", mins, secs)
    } else {
        format!("{}.{:01}s", elapsed_secs, summary.elapsed.subsec_millis() / 100)
    };

    let avg_fps = if summary.elapsed.as_secs_f64() > 0.0 && summary.frame_count > 0 {
        summary.frame_count as f64 / summary.elapsed.as_secs_f64()
    } else {
        0.0
    };

    let out_name = summary
        .output_path
        .split(std::path::MAIN_SEPARATOR)
        .last()
        .unwrap_or(&summary.output_path);

    let (status_label, status_color) = match &summary.result {
        Ok(()) => (" RENDER COMPLETE", Palette::SUCCESS),
        Err(msg) if msg == "Cancelled by user" => (" RENDER CANCELLED", Color::Yellow),
        Err(_) => (" RENDER FAILED", Color::Red),
    };

    let mut lines = vec![
        Line::from(Span::styled(
            status_label,
            Style::default().fg(status_color).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Output:      ", Style::default().fg(Palette::LABEL)),
            Span::styled(out_name, Style::default().fg(Palette::VALUE)),
        ]),
        Line::from(vec![
            Span::styled("  Codec:       ", Style::default().fg(Palette::LABEL)),
            Span::styled(
                format!("{} ({})", summary.codec_label, summary.profile_label),
                Style::default().fg(Palette::VALUE),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Gamut:       ", Style::default().fg(Palette::LABEL)),
            Span::styled(&summary.color_space, Style::default().fg(Palette::VALUE)),
        ]),
        Line::from(vec![
            Span::styled("  Transfer:    ", Style::default().fg(Palette::LABEL)),
            Span::styled(&summary.transfer, Style::default().fg(Palette::VALUE)),
        ]),
        Line::from(vec![
            Span::styled("  Rate:        ", Style::default().fg(Palette::LABEL)),
            Span::styled(&summary.rate_control, Style::default().fg(Palette::VALUE)),
        ]),
        Line::from(vec![
            Span::styled("  Frames:      ", Style::default().fg(Palette::LABEL)),
            Span::styled(format!("{}", summary.frame_count), Style::default().fg(Palette::VALUE)),
        ]),
        Line::from(vec![
            Span::styled("  Time:        ", Style::default().fg(Palette::LABEL)),
            Span::styled(elapsed_str, Style::default().fg(Palette::VALUE)),
            Span::raw("  "),
            Span::styled(
                format!("({:.1} fps avg)", avg_fps),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ];

    // Add a wrapped error message for failures so the user can see why.
    if let Err(ref msg) = summary.result {
        if msg != "Cancelled by user" {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Error:",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
            // Show up to ~3 lines of the error.
            for chunk in msg.lines().take(6) {
                lines.push(Line::from(Span::styled(
                    format!("    {}", chunk),
                    Style::default().fg(Color::Red),
                )));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press [v] or [R] to start a new export",
        Style::default().fg(Color::DarkGray),
    )));

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Render Summary ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(panel, area);
}

fn render_preview_panel(frame: &mut Frame, app: &App, area: Rect, border_color: Color) {
    let info = app.focused_file_info().or(app.file_info.as_ref());

    let text = if let Some(info) = info {
        let name = info.path.split(std::path::MAIN_SEPARATOR).last().unwrap_or(&info.path);
        let duration_secs = if info.fps > 0.0 { info.frame_count as f64 / info.fps } else { 0.0 };
        let mins = duration_secs as u64 / 60;
        let secs = duration_secs as u64 % 60;
        vec![
            Line::from(Span::styled(format!(" PREVIEW: {}", name), Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from(vec![Span::styled("  Resolution:  ", Style::default().fg(Palette::LABEL)), Span::styled(format!("{} x {}", info.width, info.height), Style::default().fg(Palette::VALUE))]),
            Line::from(vec![Span::styled("  Frames:      ", Style::default().fg(Palette::LABEL)), Span::styled(format!("{}", info.frame_count), Style::default().fg(Palette::VALUE))]),
            Line::from(vec![Span::styled("  Frame Rate:  ", Style::default().fg(Palette::LABEL)), Span::styled(format!("{:.1} fps", info.fps), Style::default().fg(Palette::VALUE))]),
            Line::from(vec![Span::styled("  Duration:    ", Style::default().fg(Palette::LABEL)), Span::styled(format!("{:02}:{:02}", mins, secs), Style::default().fg(Palette::VALUE))]),
            Line::from(vec![Span::styled("  Camera:      ", Style::default().fg(Palette::LABEL)), Span::styled(info.camera_metadata.camera_model.as_deref().unwrap_or("MotionCam"), Style::default().fg(Palette::VALUE))]),
            Line::from(vec![Span::styled("  Sensor:      ", Style::default().fg(Palette::LABEL)), Span::styled(info.camera_metadata.sensor_model.as_deref().unwrap_or("Unknown"), Style::default().fg(Palette::VALUE))]),
            Line::from(vec![Span::styled("  ISO:         ", Style::default().fg(Palette::LABEL)), Span::styled(info.camera_metadata.iso.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string()), Style::default().fg(Palette::VALUE))]),
            Line::from(Span::styled("  [Preview coming soon]", Style::default().fg(Color::DarkGray))),
        ]
    } else {
        vec![
            Line::from(Span::styled(" PREVIEW", Style::default().fg(Palette::LABEL).add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from(Span::styled("  Select a file from media pool", Style::default().fg(Color::DarkGray))),
        ]
    };

    let panel = Paragraph::new(text)
        .block(
            Block::default()
                .title(" Preview / Info ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(panel, area);
}

fn render_render_progress(frame: &mut Frame, app: &App, area: Rect, border_color: Color) {
    let pct = app.export_progress;
    let bar_width = area.width.saturating_sub(4) as usize;
    let filled = (pct / 100.0 * bar_width as f64) as usize;
    let empty = bar_width.saturating_sub(filled);

    let mut bar = String::from("[");
    for _ in 0..filled { bar.push('#'); }
    for _ in 0..empty { bar.push('-'); }
    bar.push(']');

    let elapsed = app.export_start_time
        .map(|t| t.elapsed())
        .unwrap_or_default();
    let elapsed_secs = elapsed.as_secs();
    let elapsed_mins = elapsed_secs / 60;
    let elapsed_remain = elapsed_secs % 60;
    let elapsed_str = format!("{:02}:{:02}", elapsed_mins, elapsed_remain);

    let est_total_secs = if pct > 0.0 {
        (elapsed.as_secs_f64() / pct * 100.0) as u64
    } else {
        0
    };
    let est_remaining = est_total_secs.saturating_sub(elapsed_secs);
    let est_mins = est_remaining / 60;
    let est_remain = est_remaining % 60;
    let eta_str = format!("{:02}:{:02}", est_mins, est_remain);

    let text = vec![
        Line::from(Span::styled(" RENDERING", Style::default().fg(Palette::QUEUE_RENDERING).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled(format!("  {}", bar), Style::default().fg(Palette::PROGRESS_BAR_FG))),
        Line::from(""),
        Line::from(Span::styled(format!("  {:.1}%  |  Elapsed: {}  |  ETA: {}", pct, elapsed_str, eta_str), Style::default().fg(Palette::SUCCESS).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("  Press [x] / [v] / Ctrl+X to cancel", Style::default().fg(Color::DarkGray))),
    ];

    let panel = Paragraph::new(text)
        .block(
            Block::default()
                .title(" Render Progress ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        );
    frame.render_widget(panel, area);
}

// ---------------------------------------------------------------------------
// Export settings
// ---------------------------------------------------------------------------

fn render_export_settings(frame: &mut Frame, app: &App, area: Rect, regions: &mut Vec<ClickRegion>) {
    let is_focused = app.focus_target == FocusTarget::ExportSettings;
    let border_color = if is_focused { Palette::BORDER_FOCUSED } else { Palette::BORDER };
    let show_rate = !matches!(app.export_codec_family, CodecFamily::ProRes | CodecFamily::DNxHR);

    // Panel-wide click region to focus export settings
    regions.push(ClickRegion { area, action: ClickAction::FocusExport });

    let mut lines = vec![
        Line::from(Span::styled(" Export Settings", Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD))),
        Line::from(""),
    ];

    // Active preset indicator (shown in the title and a sub-line).
    // Truncated to fit the inner panel width so a long preset name
    // (or the "(none — press P to pick or p to save current)" hint) never
    // wraps onto a second row. A wrapping preset line would silently push
    // every cycle control down by 1 row, making the touch hit regions
    // land one row above the visible control.
    let preset_label = "Preset:";
    let preset_value = match &app.active_preset {
        Some(name) => {
            let matches = app.current_matches_preset(name);
            let marker = if matches { "●" } else { "○" };
            let status = if matches { " (in sync)" } else { " (modified)" };
            format!("{} {}{}", marker, name, status)
        }
        None => "(none — press P to pick or p to save current)".to_string(),
    };
    let preset_value_display = truncate_to_width(&preset_value, max_value_width(area.width, preset_label));
    lines.push(Line::from(Span::styled(
        format!("  {} {}", preset_label, preset_value_display),
        Style::default().fg(Palette::LABEL),
    )));
    lines.push(Line::from(""));

    // The Paragraph is wrapped in Borders::ALL, so the inner content starts
    // at area.y + 1. The lines pushed above occupy rows 1..=4 (title, blank,
    // preset, blank), so the first control row (Codec) is at area.y + 5.
    let base_y = area.y + 5;

    // Click region covering the whole preset line — tapping the preset
    // (active name, sync marker, or the "(none — press P to pick …)" hint)
    // opens the preset picker, mirroring the `P` key.
    let preset_area = Rect {
        x: area.x + 1,
        y: area.y + 3,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    regions.push(ClickRegion {
        area: preset_area,
        action: ClickAction::OpenPresetPicker,
    });

    // --- Codec ---
    let co_focused = app.export_focus == ExportFocus::CodecFamily && is_focused;
    let codec_name = app.export_codec_family.name();
    let codec_style = if co_focused {
        Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD)
    } else if is_codec_hw_available(app) {
        Style::default().fg(Palette::HW_CODEC).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Palette::SW_CODEC)
    };
    let codec_suffix = if is_codec_hw_available(app) { " [HW]" } else { " [SW]" };
    let codec_value = format!("{}{}", codec_name, codec_suffix);
    let codec_display = truncate_to_width(&codec_value, max_value_width(area.width, "Codec:"));
    lines.push(Line::from(vec![
        Span::styled("  Codec:    ", Style::default().fg(Palette::LABEL)),
        Span::styled(codec_display, codec_style),
    ]));
    let co_area = Rect { x: area.x + 1, y: base_y, width: area.width.saturating_sub(2), height: 1 };
    regions.push(ClickRegion { area: co_area, action: ClickAction::CycleCodec });

    // --- Gamut ---
    let cs_focused = app.export_focus == ExportFocus::ColorSpace && is_focused;
    let gamut_display = truncate_to_width(app.export_color_space.name(), max_value_width(area.width, "Gamut:"));
    lines.push(Line::from(vec![
        Span::styled("  Gamut:    ", Style::default().fg(Palette::LABEL)),
        Span::styled(gamut_display, if cs_focused {
            Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Palette::VALUE)
        }),
    ]));
    let cs_area = Rect { x: area.x + 1, y: base_y + 1, width: area.width.saturating_sub(2), height: 1 };
    regions.push(ClickRegion { area: cs_area, action: ClickAction::CycleGamut });

    // --- Transfer ---
    let tf_focused = app.export_focus == ExportFocus::TransferFunction && is_focused;
    let tf_display = truncate_to_width(app.export_transfer_function.name(), max_value_width(area.width, "Transfer:"));
    lines.push(Line::from(vec![
        Span::styled("  Transfer: ", Style::default().fg(Palette::LABEL)),
        Span::styled(tf_display, if tf_focused {
            Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Palette::VALUE)
        }),
    ]));
    let tf_area = Rect { x: area.x + 1, y: base_y + 2, width: area.width.saturating_sub(2), height: 1 };
    regions.push(ClickRegion { area: tf_area, action: ClickAction::CycleTransfer });

    // --- Profile ---
    let pr_focused = app.export_focus == ExportFocus::Profile && is_focused;
    let profile_display = truncate_to_width(app.active_profile_name(), max_value_width(area.width, "Profile:"));
    lines.push(Line::from(vec![
        Span::styled("  Profile:  ", Style::default().fg(Palette::LABEL)),
        Span::styled(profile_display, if pr_focused {
            Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Palette::VALUE)
        }),
    ]));
    let pr_area = Rect { x: area.x + 1, y: base_y + 3, width: area.width.saturating_sub(2), height: 1 };
    regions.push(ClickRegion { area: pr_area, action: ClickAction::CycleProfile });

    // --- Rate ---
    if show_rate {
        let rc_focused = app.export_focus == ExportFocus::RateControl && is_focused;
        let rate_display = truncate_to_width(&app.active_rate_control.name(), max_value_width(area.width, "Rate:"));
        lines.push(Line::from(vec![
            Span::styled("  Rate:     ", Style::default().fg(Palette::LABEL)),
            Span::styled(rate_display, if rc_focused {
                Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Palette::VALUE)
            }),
        ]));
        let rc_area = Rect { x: area.x + 1, y: base_y + 4, width: area.width.saturating_sub(2), height: 1 };
        regions.push(ClickRegion { area: rc_area, action: ClickAction::CycleRate });
    }

    lines.push(Line::from(""));
    if let Some(ref folder) = app.export_folder {
        let disp = folder.to_string_lossy().to_string();
        let out_max = max_value_width(area.width, "OutFolder:");
        let out_display = truncate_to_width(&disp, out_max);
        lines.push(Line::from(vec![
            Span::styled("  OutFolder: ", Style::default().fg(Palette::LABEL)),
            Span::styled(out_display, Style::default().fg(Palette::VALUE)),
        ]));
    } else {
        let hint = "(default)  [o] set via browser";
        let out_max = max_value_width(area.width, "OutFolder:");
        let out_display = truncate_to_width(hint, out_max);
        lines.push(Line::from(Span::styled(
            format!("  OutFolder: {}", out_display),
            Style::default().fg(Palette::LABEL),
        )));
    }
    lines.push(Line::from(Span::styled("  [c] Codec  [g] Gamut  [t] Transfer  [r] Rate  [P] Pick preset  [p] Save preset", Style::default().fg(Color::White))));

    let panel = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Export Config ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(panel, area);
}

fn is_codec_hw_available(app: &App) -> bool {
    match app.export_codec_family {
        CodecFamily::HEVC => app.hardware_caps.hevc_is_hw,
        CodecFamily::H264 => app.hardware_caps.h264_is_hw,
        CodecFamily::AV1 => app.hardware_caps.av1_is_hw,
        CodecFamily::ProRes => app.hardware_caps.prores_is_hw,
        CodecFamily::DNxHR | CodecFamily::VP9 => false,
    }
}

/// Maximum display width available for a value on a control row, accounting
/// for the row's 2-space indent, label, padding, and the 1-col inner border
/// on each side of the panel. Returns at least 1 so a single char always fits.
fn max_value_width(panel_width: u16, label: &str) -> usize {
    // panel_width includes both borders; inner is panel_width - 2.
    // Row uses 2-space indent + label + 1-space minimum separator.
    let inner = panel_width.saturating_sub(2) as usize;
    let reserved = 2 + label.chars().count() + 1;
    inner.saturating_sub(reserved).max(1)
}

/// Truncate `s` to at most `max_chars` characters, appending an ellipsis
/// when truncation happens so the user sees the value was clipped (not
/// silently cut off mid-word).
fn truncate_to_width(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let keep = max_chars - 1;
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// Render queue
// ---------------------------------------------------------------------------

fn render_queue_panel(frame: &mut Frame, app: &App, area: Rect, regions: &mut Vec<ClickRegion>) {
    let is_focused = app.focus_target == FocusTarget::Queue;
    let border_color = if is_focused { Palette::BORDER_FOCUSED } else { Palette::BORDER };
    let inner_h = area.height.saturating_sub(2) as usize;

    // Panel-wide click region to focus queue
    regions.push(ClickRegion { area, action: ClickAction::FocusQueue });

    if app.queue.is_empty() {
        let placeholder = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("  No jobs in queue", Style::default().fg(Color::DarkGray))),
            Line::from(Span::styled("  Select files and press [a] to add", Style::default().fg(Color::DarkGray))),
        ]).block(
            Block::default()
                .title(" Render Queue ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        );
        frame.render_widget(placeholder, area);
    } else {
        let items: Vec<ListItem> = app.queue.iter().enumerate().map(|(_i, q)| {
            let name = q.path.split(std::path::MAIN_SEPARATOR).last().unwrap_or(&q.path);
            let checkbox = if q.selected {
                Span::styled("[*] ", Style::default().fg(Palette::CHECKED).add_modifier(Modifier::BOLD))
            } else {
                Span::styled("[ ] ", Style::default().fg(Palette::UNCHECKED))
            };
            let (status_color, status_text) = match &q.status {
                QueueStatus::Waiting => (Palette::QUEUE_WAITING, "Waiting"),
                QueueStatus::Rendering => (Palette::QUEUE_RENDERING, "Rendering"),
                QueueStatus::Completed => (Palette::QUEUE_COMPLETED, "Done"),
                QueueStatus::Failed(_) => (Palette::QUEUE_FAILED, "Failed"),
            };
            let progress_str = if matches!(q.status, QueueStatus::Rendering) {
                format!("{:.0}%", q.progress)
            } else {
                status_text.to_string()
            };
            let content = Line::from(vec![
                checkbox,
                Span::styled(name, Style::default().fg(Color::White)),
                Span::raw("  "),
                Span::styled(app.export_codec_family.name(), Style::default().fg(Color::Cyan)),
                Span::raw("  "),
                Span::styled(progress_str, Style::default().fg(status_color)),
            ]);
            ListItem::new(content)
        }).collect();

        let item_count = app.queue.len();

        // Calculate visible items and scroll offset
        let has_room_for_buttons = inner_h >= 3;
        let visible_items = if has_room_for_buttons { inner_h - 1 } else { inner_h };

        let list = List::new(items)
            .block(
                Block::default()
                    .title(format!(" Render Queue ({}) ", app.queue.len()))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border_color)),
            )
            .highlight_style(
                if is_focused {
                    Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD).bg(Palette::HIGHLIGHT_FOCUSED_BG)
                } else {
                    Style::default().fg(Color::White).bg(Palette::HIGHLIGHT_BG)
                },
            )
            .highlight_symbol("> ");

        let mut state = ListState::default();
        state.select(Some(app.queue_index));
        frame.render_stateful_widget(list, area, &mut state);

        // Calculate scroll offset to match List widget behavior
        let visible_start = if app.queue_index >= visible_items {
            app.queue_index - visible_items + 1
        } else {
            0
        };

        for i in 0..visible_items.min(item_count) {
            let entry_index = visible_start + i;
            if entry_index >= item_count {
                break;
            }
            let row_y = area.y + 1 + i as u16;
            let cb_area = Rect { x: area.x + 2, y: row_y, width: 4, height: 1 };
            regions.push(ClickRegion { area: cb_area, action: ClickAction::ToggleQueueSelection(entry_index) });
            let row_area = Rect { x: area.x + 6, y: row_y, width: area.width.saturating_sub(8), height: 1 };
            regions.push(ClickRegion { area: row_area, action: ClickAction::SelectQueueItem(entry_index) });
        }

        // Render buttons at the bottom if there's room
        if has_room_for_buttons {
            let btn_y = area.y + area.height.saturating_sub(2);
            let btn_row = Rect {
                x: area.x + 2,
                y: btn_y,
                width: area.width.saturating_sub(4),
                height: 1,
            };

            let render_btn = Rect { x: btn_row.x, y: btn_row.y, width: 12, height: 1 };
            regions.push(ClickRegion { area: render_btn, action: ClickAction::RenderSelected });

            let all_btn = Rect { x: btn_row.x + 13, y: btn_row.y, width: 8, height: 1 };
            regions.push(ClickRegion { area: all_btn, action: ClickAction::RenderAll });

            let clear_btn = Rect { x: btn_row.x + 22, y: btn_row.y, width: 10, height: 1 };
            regions.push(ClickRegion { area: clear_btn, action: ClickAction::ClearQueue });

            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" [v] Render ", Style::default().fg(Palette::BUTTON_FG).bg(Palette::BUTTON_BG).add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::styled(" [R] All ", Style::default().fg(Palette::BUTTON_FG).bg(Palette::BUTTON_BG).add_modifier(Modifier::BOLD)),
                    Span::raw(" "),
                    Span::styled(" [x] Clear ", Style::default().fg(Palette::BUTTON_FG).bg(Palette::BUTTON_BG).add_modifier(Modifier::BOLD)),
                ])),
                btn_row,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn render_status(frame: &mut Frame, app: &App, area: Rect, regions: &mut Vec<ClickRegion>) {
    let mut hints = vec![
        Span::styled("[b]", Style::default().fg(Palette::STATUS_KEY)),
        Span::styled(" Browser  ", Style::default().fg(Color::White)),
        Span::styled("[Space]", Style::default().fg(Palette::STATUS_KEY)),
        Span::styled(" Select  ", Style::default().fg(Color::White)),
        Span::styled("[a]", Style::default().fg(Palette::STATUS_KEY)),
        Span::styled(" Add  ", Style::default().fg(Color::White)),
        Span::styled("[Tab]", Style::default().fg(Palette::STATUS_KEY)),
        Span::styled(" Panel  ", Style::default().fg(Color::White)),
        Span::styled("[v]", Style::default().fg(Palette::STATUS_KEY)),
        Span::styled(" Render  ", Style::default().fg(Color::White)),
        Span::styled("[?]", Style::default().fg(Palette::STATUS_KEY)),
        Span::styled(" Help  ", Style::default().fg(Color::White)),
        Span::styled("[C]", Style::default().fg(Palette::STATUS_KEY)),
        Span::styled(" Culling  ", Style::default().fg(Color::White)),
    ];
    if app.show_browser {
        hints.push(Span::styled("[I]", Style::default().fg(Palette::STATUS_KEY)));
        hints.push(Span::styled(" Import  ", Style::default().fg(Color::White)));
        hints.push(Span::styled("[L]", Style::default().fg(Palette::STATUS_KEY)));
        hints.push(Span::styled(" Load All  ", Style::default().fg(Color::White)));
        hints.push(Span::styled("[o]", Style::default().fg(Palette::STATUS_KEY)));
        hints.push(Span::styled(" OutFolder  ", Style::default().fg(Color::White)));
        hints.push(Span::styled("[F]", Style::default().fg(Palette::STATUS_KEY)));
        hints.push(Span::styled(" Fav  ", Style::default().fg(Color::White)));
    }

    let msg = if !app.status_message.is_empty() {
        format!(" {} | ", app.status_message)
    } else {
        String::new()
    };
    let mut all_spans = vec![Span::styled(msg, Style::default().fg(Color::White))];
    all_spans.extend(hints);

    // Visual feedback: flash status bar border green briefly after a drop
    let border_color = if let Some(drop_time) = app.drop_highlight {
        if drop_time.elapsed() < Duration::from_millis(800) {
            Color::Green
        } else {
            Palette::BORDER
        }
    } else {
        Palette::BORDER
    };

    let status = Paragraph::new(Line::from(all_spans))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        );
    frame.render_widget(status, area);
}

// ---------------------------------------------------------------------------
// Import popup
// ---------------------------------------------------------------------------

fn render_import_popup(frame: &mut Frame, area: Rect, app: &App, regions: &mut Vec<ClickRegion>) {
    let popup_area = centered_rect(65, 45, area);
    frame.render_widget(Clear, popup_area);

    let mut lines = vec![
        Line::from(Span::styled(" Import .mcraw files", Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD))),
        Line::from(""),
    ];

    let mut opt1_idx: Option<usize> = None;
    let mut opt2_idx: Option<usize> = None;

    if let ImportPopupState::DroppedFiles { files, folder, all_in_folder } = &app.import_popup {
        let dropped_count = files.len();
        let folder_count = all_in_folder.len();
        let has_option2 = folder_count > dropped_count;

        // Show dropped file names (up to 3)
        if dropped_count == 1 {
            let name = files[0].split(std::path::MAIN_SEPARATOR).last().unwrap_or(&files[0]);
            lines.push(Line::from(Span::styled(format!("  Dropped: {}", name), Style::default().fg(Palette::VALUE))));
        } else {
            lines.push(Line::from(Span::styled(format!("  Dropped: {} file(s)", dropped_count), Style::default().fg(Palette::VALUE))));
            for path in files.iter().take(3) {
                let name = path.split(std::path::MAIN_SEPARATOR).last().unwrap_or(path);
                lines.push(Line::from(Span::styled(format!("    - {}", name), Style::default().fg(Color::Gray))));
            }
            if dropped_count > 3 {
                lines.push(Line::from(Span::styled(format!("    ... and {} more", dropped_count - 3), Style::default().fg(Color::DarkGray))));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!("  Folder: {}", folder), Style::default().fg(Color::DarkGray))));
        lines.push(Line::from(Span::styled(format!("  Total in folder: {} .mcraw files", folder_count), Style::default().fg(Color::DarkGray))));
        lines.push(Line::from(""));

        lines.push(Line::from(Span::styled("  [1] Import dropped file(s) only", Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD))));
        opt1_idx = Some(lines.len() - 1);

        if has_option2 {
            lines.push(Line::from(Span::styled(format!("  [2] Import all {} file(s) in folder", folder_count), Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD))));
            opt2_idx = Some(lines.len() - 1);
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Click, Enter, or 1/2 to select", Style::default().fg(Color::DarkGray))));
    }

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Import ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Palette::POPUP_BORDER)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(popup, popup_area);

    // The Paragraph is wrapped in Borders::ALL, so the first line of content
    // is at `popup_area.y + 1`. Derive each option's y from the line index
    // recorded above so the click target always lands on the rendered text,
    // regardless of how many dropped-file rows were pushed beforehand.
    if let Some(idx) = opt1_idx {
        regions.push(ClickRegion {
            area: Rect {
                x: popup_area.x + 2,
                y: popup_area.y + 1 + idx as u16,
                width: popup_area.width.saturating_sub(4),
                height: 1,
            },
            action: ClickAction::ImportOption1,
        });
    }

    if let Some(idx) = opt2_idx {
        regions.push(ClickRegion {
            area: Rect {
                x: popup_area.x + 2,
                y: popup_area.y + 1 + idx as u16,
                width: popup_area.width.saturating_sub(4),
                height: 1,
            },
            action: ClickAction::ImportOption2,
        });
    }
}

// ---------------------------------------------------------------------------
// Drop preview overlay
// ---------------------------------------------------------------------------

fn render_drop_preview(frame: &mut Frame, area: Rect, preview: &crate::app::DropPreview) {
    let elapsed = preview.start_time.elapsed();
    if elapsed >= Duration::from_secs(2) {
        return;
    }

    // Calculate fade-out in last 500ms
    let alpha = if elapsed > Duration::from_millis(1500) {
        1.0 - ((elapsed.as_millis() - 1500) as f32 / 500.0)
    } else {
        1.0
    };

    let popup_area = centered_rect(50, 25.min(15 + preview.files.len() as u16), area);
    frame.render_widget(Clear, popup_area);

    let mut lines = vec![
        Line::from(Span::styled(
            " Files Dropped",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // Show up to 5 file names
    let max_show = 5.min(preview.files.len());
    for (i, file) in preview.files.iter().take(max_show).enumerate() {
        let name = file.split(std::path::MAIN_SEPARATOR).last().unwrap_or(file);
        let icon = if i < max_show - 1 || preview.files.len() <= max_show {
            "  ✓ "
        } else {
            "  ✓ "
        };
        lines.push(Line::from(vec![
            Span::styled(icon, Style::default().fg(Color::Green)),
            Span::styled(name, Style::default().fg(Color::White)),
        ]));
    }

    if preview.files.len() > max_show {
        lines.push(Line::from(Span::styled(
            format!("    ... and {} more", preview.files.len() - max_show),
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Importing...",
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
    )));

    let border_color = if alpha > 0.5 { Color::Green } else { Color::DarkGray };

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Drop ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        )
        .wrap(Wrap { trim: false })
        .alignment(Alignment::Left);
    frame.render_widget(popup, popup_area);
}

// ---------------------------------------------------------------------------
// Full info overlay
// ---------------------------------------------------------------------------

fn render_full_info_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let popup_area = centered_rect(75, 80, area);
    frame.render_widget(Clear, popup_area);

    let info = app.focused_file_info().or(app.file_info.as_ref());

    let lines = if let Some(info) = info {
        let mut lines = Vec::new();

        // General section
        lines.push(Line::from(Span::styled(
            " General",
            Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD),
        )));
        let filename = info.path.split(std::path::MAIN_SEPARATOR).last().unwrap_or(&info.path);
        lines.push(Line::from(vec![
            Span::styled("  Filename:     ", Style::default().fg(Palette::LABEL)),
            Span::styled(filename, Style::default().fg(Palette::VALUE)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Path:         ", Style::default().fg(Palette::LABEL)),
            Span::styled(&info.path, Style::default().fg(Palette::VALUE)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Size:         ", Style::default().fg(Palette::LABEL)),
            Span::styled(format_size(info.size), Style::default().fg(Palette::VALUE)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Format:       ", Style::default().fg(Palette::LABEL)),
            Span::styled(info.format_name(), Style::default().fg(Palette::VALUE)),
        ]));
        if let Some(ref date) = info.camera_metadata.capture_date {
            lines.push(Line::from(vec![
                Span::styled("  Capture Date: ", Style::default().fg(Palette::LABEL)),
                Span::styled(format_capture_date(date), Style::default().fg(Palette::VALUE)),
            ]));
        }
        lines.push(Line::from(""));

        // Camera section
        lines.push(Line::from(Span::styled(
            " Camera",
            Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD),
        )));
        if let Some(ref model) = info.camera_metadata.camera_model {
            lines.push(Line::from(vec![
                Span::styled("  Camera:       ", Style::default().fg(Palette::LABEL)),
                Span::styled(model, Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(ref sensor_make) = info.camera_metadata.sensor_make {
            lines.push(Line::from(vec![
                Span::styled("  Sensor Make:  ", Style::default().fg(Palette::LABEL)),
                Span::styled(sensor_make, Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(ref sensor_model) = info.camera_metadata.sensor_model {
            let make = info.camera_metadata.sensor_make.as_deref().unwrap_or("");
            lines.push(Line::from(vec![
                Span::styled("  Sensor:       ", Style::default().fg(Palette::LABEL)),
                Span::styled(format!("{} {}", make, sensor_model), Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(ref lens) = info.camera_metadata.lens_model {
            lines.push(Line::from(vec![
                Span::styled("  Lens:         ", Style::default().fg(Palette::LABEL)),
                Span::styled(lens, Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(fl) = info.camera_metadata.focal_length {
            lines.push(Line::from(vec![
                Span::styled("  Focal Length: ", Style::default().fg(Palette::LABEL)),
                Span::styled(format!("{:.1}mm", fl), Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(ap) = info.camera_metadata.aperture {
            lines.push(Line::from(vec![
                Span::styled("  Aperture:     ", Style::default().fg(Palette::LABEL)),
                Span::styled(format!("f/{:.1}", ap), Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(iso) = info.camera_metadata.iso {
            lines.push(Line::from(vec![
                Span::styled("  ISO:          ", Style::default().fg(Palette::LABEL)),
                Span::styled(iso.to_string(), Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(et) = info.camera_metadata.exposure_time {
            lines.push(Line::from(vec![
                Span::styled("  Exposure:     ", Style::default().fg(Palette::LABEL)),
                Span::styled(format_exposure_time(et), Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(wb) = info.camera_metadata.white_balance {
            lines.push(Line::from(vec![
                Span::styled("  White Balance:", Style::default().fg(Palette::LABEL)),
                Span::styled(format!("{:.0}K", wb), Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(ref cm) = info.camera_metadata.color_matrix {
            let vals: Vec<String> = cm.iter().map(|v| format!("{:.2}", v)).collect();
            lines.push(Line::from(vec![
                Span::styled("  Color Matrix1:", Style::default().fg(Palette::LABEL)),
                Span::styled(format!("[{}]", vals.join(", ")), Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(ref cm) = info.camera_metadata.color_matrix2 {
            let vals: Vec<String> = cm.iter().map(|v| format!("{:.2}", v)).collect();
            lines.push(Line::from(vec![
                Span::styled("  Color Matrix2:", Style::default().fg(Palette::LABEL)),
                Span::styled(format!("[{}]", vals.join(", ")), Style::default().fg(Palette::VALUE)),
            ]));
        }
        if let Some(i1) = info.camera_metadata.calibration_illuminant1 {
            if let Some(i2) = info.camera_metadata.calibration_illuminant2 {
                lines.push(Line::from(vec![
                    Span::styled("  Cal Illuminants:", Style::default().fg(Palette::LABEL)),
                    Span::styled(format!("{} / {}", i1, i2), Style::default().fg(Palette::VALUE)),
                ]));
            }
        }
        lines.push(Line::from(""));

        // Video section
        lines.push(Line::from(Span::styled(
            " Video",
            Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(vec![
            Span::styled("  Resolution:   ", Style::default().fg(Palette::LABEL)),
            Span::styled(format!("{}x{} ({})", info.width, info.height, info.resolution_label()), Style::default().fg(Palette::VALUE)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  FPS:          ", Style::default().fg(Palette::LABEL)),
            Span::styled(format!("{:.2}", info.fps), Style::default().fg(Palette::VALUE)),
        ]));
        let duration_secs = if info.fps > 0.0 { info.frame_count as f64 / info.fps } else { 0.0 };
        lines.push(Line::from(vec![
            Span::styled("  Duration:     ", Style::default().fg(Palette::LABEL)),
            Span::styled(format_duration(duration_secs), Style::default().fg(Palette::VALUE)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Frames:       ", Style::default().fg(Palette::LABEL)),
            Span::styled(info.frame_count.to_string(), Style::default().fg(Palette::VALUE)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Bit Depth:    ", Style::default().fg(Palette::LABEL)),
            Span::styled(format!("{}-bit", info.bit_depth), Style::default().fg(Palette::VALUE)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Bayer:        ", Style::default().fg(Palette::LABEL)),
            Span::styled(info.bayer_pattern.name(), Style::default().fg(Palette::VALUE)),
        ]));
        if info.sensor_width > 0 || info.sensor_height > 0 {
            lines.push(Line::from(vec![
                Span::styled("  Sensor Size:  ", Style::default().fg(Palette::LABEL)),
                Span::styled(format!("{}x{}", info.sensor_width, info.sensor_height), Style::default().fg(Palette::VALUE)),
            ]));
        }
        if info.active_width > 0 && info.active_height > 0 {
            lines.push(Line::from(vec![
                Span::styled("  Active Area:  ", Style::default().fg(Palette::LABEL)),
                Span::styled(format!("{}x{} @({},{})", info.active_width, info.active_height, info.active_offset_x, info.active_offset_y), Style::default().fg(Palette::VALUE)),
            ]));
        }
        lines.push(Line::from(""));

        // Audio section
        lines.push(Line::from(Span::styled(
            " Audio",
            Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD),
        )));
        if info.has_audio {
            lines.push(Line::from(vec![
                Span::styled("  Has Audio:    ", Style::default().fg(Palette::LABEL)),
                Span::styled("Yes", Style::default().fg(Palette::VALUE)),
            ]));
            if info.audio_sample_rate > 0 {
                lines.push(Line::from(vec![
                    Span::styled("  Sample Rate:  ", Style::default().fg(Palette::LABEL)),
                    Span::styled(format!("{} Hz", info.audio_sample_rate), Style::default().fg(Palette::VALUE)),
                ]));
            }
            if info.audio_channels > 0 {
                let ch_name = if info.audio_channels == 1 {
                    "mono"
                } else if info.audio_channels == 2 {
                    "stereo"
                } else {
                    "multi"
                };
                lines.push(Line::from(vec![
                    Span::styled("  Channels:     ", Style::default().fg(Palette::LABEL)),
                    Span::styled(format!("{} ({})", info.audio_channels, ch_name), Style::default().fg(Palette::VALUE)),
                ]));
            }
            if let Some(length) = info.audio_length {
                lines.push(Line::from(vec![
                    Span::styled("  Audio Length: ", Style::default().fg(Palette::LABEL)),
                    Span::styled(format!("{} bytes", length), Style::default().fg(Palette::VALUE)),
                ]));
            }
            if let Some(offset) = info.audio_offset {
                lines.push(Line::from(vec![
                    Span::styled("  Audio Offset: ", Style::default().fg(Palette::LABEL)),
                    Span::styled(format!("{} bytes", offset), Style::default().fg(Palette::VALUE)),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("  Has Audio:    ", Style::default().fg(Palette::LABEL)),
                Span::styled("No", Style::default().fg(Palette::VALUE)),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Press [i] or Esc to close", Style::default().fg(Color::DarkGray))));

        lines
    } else {
        vec![
            Line::from(Span::styled(" FILE INFO", Style::default().fg(Palette::LABEL).add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from(Span::styled("  No file selected", Style::default().fg(Color::DarkGray))),
            Line::from(""),
            Line::from(Span::styled("  Press [i] or Esc to close", Style::default().fg(Color::DarkGray))),
        ]
    };

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Full File Info (Esc/i to close) ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Palette::POPUP_BORDER)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(popup, popup_area);
}

// ---------------------------------------------------------------------------
// Help overlay
// ---------------------------------------------------------------------------

fn render_help_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let popup_area = centered_rect(70, 70, area);
    frame.render_widget(Clear, popup_area);

    let all_lines = vec![
        Line::from(Span::styled(" Keybindings", Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("  Navigation", Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  b          Toggle browser overlay", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  Tab        Cycle focus: Media Pool -> Preview -> Export -> Queue", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  Click      Click panel or items to focus/select", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  Scroll     Scroll wheel navigates the hovered panel", Style::default().fg(Palette::VALUE))),
        Line::from(""),
        Line::from(Span::styled("  Media Pool", Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  Space      Toggle selection checkbox", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  a          Add selected to render queue", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  A          Add ALL to render queue", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  d          Remove current from media pool", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  D          Remove ALL selected from media pool", Style::default().fg(Palette::VALUE))),
        Line::from(""),
        Line::from(Span::styled("  Render Queue", Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  Space      Toggle selection in queue", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  v          Render selected items", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  R          Render ALL items (sequential batch)", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  x          Clear completed/failed", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  d          Remove from queue", Style::default().fg(Palette::VALUE))),
        Line::from(""),
        Line::from(Span::styled("  Export Settings", Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  e          Focus export settings", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  c/g/t/r    Cycle codec/gamut/transfer/rate", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  P          Open preset picker (apply saved preset)", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  p          Save current settings as preset", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  i          Edit custom rate (when export focused)", Style::default().fg(Palette::VALUE))),
        Line::from(""),
        Line::from(Span::styled("  Browser", Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  Click/Dbl  Select/Open file/folder", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  Enter      Open selected file/folder", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  Space      Toggle selection checkbox", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  I          Import selected .mcraw", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  L          Load all .mcraw in folder", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  o          Set export folder to browser path", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  F          Toggle favourite folder (current)", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  f          Toggle favourites list view (keyboard nav)", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  Delete     Remove selected favourite (in list view)", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  .          Toggle hidden files", Style::default().fg(Palette::VALUE))),
        Line::from(""),
        Line::from(Span::styled("  Culling", Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  C          Toggle culling mode", Style::default().fg(Palette::VALUE))),
        Line::from(""),
        Line::from(Span::styled("  File Info / Preview", Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  i          Show full file info for selected file", Style::default().fg(Palette::VALUE))),
        Line::from(""),
        Line::from(Span::styled("  General", Style::default().fg(Palette::FOCUSED).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  q          Quit", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  ?          Toggle this help", Style::default().fg(Palette::VALUE))),
        Line::from(Span::styled("  Esc        Close popup/browser/help -> Quit", Style::default().fg(Palette::VALUE))),
        Line::from(""),
        Line::from(Span::styled("  Codec colors: [HW] green = hardware accelerated", Style::default().fg(Palette::HW_CODEC))),
        Line::from(Span::styled("                  [SW] orange = software encoder", Style::default().fg(Palette::SW_CODEC))),
        Line::from(""),
        Line::from(Span::styled("  Logs: stored in app data directory, auto-cleaned after 7 days", Style::default().fg(Color::DarkGray))),
        Line::from(Span::styled("  Drag & drop .mcraw files onto the terminal to import", Style::default().fg(Color::DarkGray))),
        Line::from(Span::styled("  ↑/↓, PageUp/Dn, Scroll wheel  Scroll this help", Style::default().fg(Color::DarkGray))),
    ];

    let inner_h = popup_area.height.saturating_sub(2) as usize;
    let scroll = app.help_scroll as usize;
    let visible: Vec<Line> = all_lines.iter()
        .skip(scroll)
        .take(inner_h)
        .cloned()
        .collect();

    let popup = Paragraph::new(visible)
        .block(
            Block::default()
                .title(format!(" Help ({}/{}) Esc to close ", scroll + 1, all_lines.len()))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Palette::POPUP_BORDER)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(popup, popup_area);
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn format_size(bytes: u64) -> String {
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

fn format_duration(seconds: f64) -> String {
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

fn format_exposure_time(value: f64) -> String {
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

fn format_capture_date(raw: &str) -> String {
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

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// ---------------------------------------------------------------------------
// Preset picker
// ---------------------------------------------------------------------------

fn render_preset_picker(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered_rect(70, 70, area);
    frame.render_widget(Clear, popup);

    let total = app.presets.len();
    let title = if total == 0 {
        " Presets (none saved — press p in Export Settings to save current) ".to_string()
    } else {
        format!(" Presets ({}) — Enter applies · Delete removes · Esc closes ", total)
    };

    let mut lines: Vec<Line> = Vec::new();
    if total == 0 {
        lines.push(Line::from(Span::styled(
            "  No presets yet.",
            Style::default().fg(Palette::LABEL),
        )));
        lines.push(Line::from(Span::styled(
            "  Focus the Export Settings panel and press [p] to save the current configuration.",
            Style::default().fg(Palette::LABEL),
        )));
        lines.push(Line::from(""));
    } else {
        for (i, p) in app.presets.iter().enumerate() {
            let is_sel = i == app.preset_picker.index;
            let marker = if is_sel { "> " } else { "  " };
            let active = app.active_preset.as_deref() == Some(p.name.as_str());
            let synced = app.current_matches_preset(&p.name);
            let dot = if active && synced { "●" } else if active { "○" } else { " " };
            let summary = format!(
                "{} · {} · {}",
                p.codec_family.name(),
                p.color_space.name(),
                p.transfer_function.name()
            );
            let rate = p.rate_control.name();
            let name_style = if is_sel {
                Style::default()
                    .fg(Palette::FOCUSED)
                    .add_modifier(Modifier::BOLD)
                    .bg(Palette::HIGHLIGHT_BG)
            } else {
                Style::default().fg(Palette::VALUE).add_modifier(Modifier::BOLD)
            };
            let meta_style = if is_sel {
                Style::default().fg(Palette::FOCUSED).bg(Palette::HIGHLIGHT_BG)
            } else {
                Style::default().fg(Palette::LABEL)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{}{} ", marker, dot), name_style),
                Span::styled(format!("{:<20}", truncate(&p.name, 20)), name_style),
                Span::styled(format!("{:<40}", truncate(&summary, 40)), meta_style),
                Span::styled(truncate(&rate, 18), meta_style),
            ]));
        }
        lines.push(Line::from(""));
        if let Some(p) = app.presets.get(app.preset_picker.index) {
            lines.push(Line::from(vec![
                Span::styled("  Codec: ", Style::default().fg(Palette::LABEL)),
                Span::styled(p.codec_family.name(), Style::default().fg(Palette::VALUE)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Gamut: ", Style::default().fg(Palette::LABEL)),
                Span::styled(p.color_space.name(), Style::default().fg(Palette::VALUE)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Trans: ", Style::default().fg(Palette::LABEL)),
                Span::styled(p.transfer_function.name(), Style::default().fg(Palette::VALUE)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  Rate:  ", Style::default().fg(Palette::LABEL)),
                Span::styled(p.rate_control.name(), Style::default().fg(Palette::VALUE)),
            ]));
            if let Some(folder) = &p.export_folder {
                let disp = folder.display().to_string();
                let trimmed = if disp.len() > 60 {
                    format!("…{}", &disp[disp.len().saturating_sub(59)..])
                } else {
                    disp
                };
                lines.push(Line::from(vec![
                    Span::styled("  Out:   ", Style::default().fg(Palette::LABEL)),
                    Span::styled(trimmed, Style::default().fg(Palette::VALUE)),
                ]));
            }
        }
    }

    lines.push(Line::from(""));
    if let Some(ref msg) = app.preset_picker.message {
        lines.push(Line::from(Span::styled(
            format!("  {}", msg),
            Style::default().fg(Palette::SUCCESS),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "  ↑/↓ navigate · Enter apply · Delete remove · Esc close",
            Style::default().fg(Palette::LABEL),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Palette::BORDER_FOCUSED))
                .title_style(Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn render_preset_naming(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered_rect(60, 25, area);
    frame.render_widget(Clear, popup);

    let naming = app.preset_naming.as_ref().expect("naming state set");
    let display_name = if naming.name.is_empty() { " ".to_string() } else { naming.name.clone() };

    let lines = vec![
        Line::from(Span::styled("  Save current export settings as preset", Style::default().fg(Palette::POPUP_TITLE).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("  Name:", Style::default().fg(Palette::LABEL))),
        Line::from(Span::styled(
            format!("  > {}_", display_name),
            Style::default().fg(Palette::VALUE).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Summary (saved into preset):",
            Style::default().fg(Palette::LABEL),
        )),
        Line::from(Span::styled(
            format!("    {} · {} · {} · {}",
                app.export_codec_family.name(),
                app.export_color_space.name(),
                app.export_transfer_function.name(),
                app.active_rate_control.name(),
            ),
            Style::default().fg(Palette::VALUE),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter to save · Esc to cancel",
            Style::default().fg(Palette::LABEL),
        )),
    ];

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Save Preset ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Palette::BORDER_FOCUSED)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max <= 1 {
        "…".to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}
