use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Tabs, Wrap},
    Frame,
};

use crate::app::{App, ExportFocus, Screen};
use crate::encoder::EncodeStatus;

pub fn render(frame: &mut Frame, app: &App) {
    let size = frame.area();

    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(size);

    render_title(frame, vert[0], app);
    render_body(frame, vert[1], app);
    render_status(frame, app, vert[2]);
}

fn render_title(frame: &mut Frame, area: Rect, app: &App) {
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " MCRAW TUI ",
            Style::default().fg(Color::White).bg(Color::Blue),
        ),
        Span::styled(
            " MotionCam Browser",
            Style::default().fg(Color::White),
        ),
        Span::styled(
            if app.is_exporting {
                format!("  [Exporting: {:.0}%]", app.export_progress)
            } else {
                String::new()
            },
            Style::default().fg(Color::Green),
        ),
    ]))
    .block(Block::default().borders(Borders::NONE));
    frame.render_widget(title, area);
}

fn render_body(frame: &mut Frame, area: Rect, app: &App) {
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(35),
            Constraint::Percentage(40),
            Constraint::Percentage(25),
        ])
        .split(area);

    if panes.len() >= 3 {
        render_browse_pane(frame, app, panes[0]);
        render_center_pane(frame, app, panes[1]);
        render_right_pane(frame, app, panes[2]);
    }
}

fn render_browse_pane(frame: &mut Frame, app: &App, area: Rect) {
    let path_display = app.browser.current_path_display();
    let items: Vec<ListItem> = app
        .browser
        .entries
        .iter()
        .map(|entry| {
            let icon = if entry.is_dir { "📁 " } else { "📄 " };
            let name_style = if entry.is_dir {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::White)
            };

            let mut content = vec![
                Span::raw(icon),
                Span::styled(&entry.name, name_style),
            ];

            if let Some(ref info) = entry.file_info {
                content.push(Span::raw("  "));
                content.push(Span::styled(
                    format!("{}x{}", info.width, info.height),
                    Style::default().fg(Color::Green),
                ));
            }

            ListItem::new(Line::from(content))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().title(format!(" Browse: {} ", path_display)).borders(Borders::ALL))
        .style(Style::default().fg(Color::Gray))
        .highlight_style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
        .highlight_symbol("▸ ");

    frame.render_stateful_widget(list, area, &mut app.list_state.clone());
}

fn render_center_pane(frame: &mut Frame, app: &App, area: Rect) {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(area);

    let tab_titles = vec![" Info ", " Export ", " Settings "];
    let selected = match app.screen {
        Screen::Info => 0,
        Screen::Export => 1,
        Screen::Browse => 0,
    };
    let tabs = Tabs::new(tab_titles.iter().map(|t| Span::styled(*t, Style::default().fg(Color::White))))
        .block(Block::default().borders(Borders::ALL))
        .select(selected)
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
    frame.render_widget(tabs, vert[0]);

    match app.screen {
        Screen::Info | Screen::Browse => {
            render_info_content(frame, app, vert[1]);
        }
        Screen::Export => {
            render_export_content(frame, app, vert[1]);
        }
    }
}

fn render_info_content(frame: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(ref info) = app.file_info {
        crate::metadata::format_metadata_for_display(info)
    } else {
        vec![
            Line::from(Span::styled(
                "No file loaded",
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Browse and open a .mcraw file",
                Style::default().fg(Color::Gray),
            )),
        ]
    };

    let panel = Paragraph::new(text)
        .block(Block::default().title(" File Info ").borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(panel, area);
}

fn render_export_content(frame: &mut Frame, app: &App, area: Rect) {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(area);

    // --- Color Space selector ---
    let cs_focused = app.export_focus == ExportFocus::ColorSpace;
    let cs_style = if cs_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let cs_line = Line::from(vec![
        Span::styled(" Gamut [g]:  ", Style::default().fg(Color::Gray)),
        Span::styled(app.export_color_space.name(), cs_style),
        Span::styled("  <Up/Down>", Style::default().fg(if cs_focused { Color::Green } else { Color::DarkGray })),
    ]);
    let cs_block = Block::default().borders(Borders::ALL);
    frame.render_widget(Paragraph::new(cs_line).block(cs_block), vert[0]);

    // --- Transfer Function selector ---
    let tf_focused = app.export_focus == ExportFocus::TransferFunction;
    let tf_style = if tf_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let tf_line = Line::from(vec![
        Span::styled(" Transfer [t]: ", Style::default().fg(Color::Gray)),
        Span::styled(app.export_transfer_function.name(), tf_style),
        Span::styled("  <Up/Down>", Style::default().fg(if tf_focused { Color::Green } else { Color::DarkGray })),
    ]);
    let tf_block = Block::default().borders(Borders::ALL);
    frame.render_widget(Paragraph::new(tf_line).block(tf_block), vert[1]);

    // --- Codec Family selector ---
    let co_focused = app.export_focus == ExportFocus::CodecFamily;
    let co_style = if co_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let co_line = Line::from(vec![
        Span::styled(" Codec [c]:  ", Style::default().fg(Color::Gray)),
        Span::styled(app.export_codec_family.name(), co_style),
        Span::styled("  <Up/Down>", Style::default().fg(if co_focused { Color::Green } else { Color::DarkGray })),
    ]);
    let co_block = Block::default().borders(Borders::ALL);
    frame.render_widget(Paragraph::new(co_line).block(co_block), vert[2]);

    // --- Profile selector (dynamic based on active codec) ---
    let pr_focused = app.export_focus == ExportFocus::Profile;
    let show_profile = !matches!(app.export_codec_family, crate::export::CodecFamily::CinemaDNG);
    let profile_conflict = show_profile && app.export_transfer_function.requires_10bit() && app.active_profile_is_8bit();
    let pr_style = if pr_focused {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else if profile_conflict {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };
    let pr_name = if show_profile {
        let name = app.active_profile_name();
        if profile_conflict {
            format!("{} (Requires 10-bit)", name)
        } else {
            name.to_string()
        }
    } else {
        "—".to_string()
    };
    let pr_line = Line::from(vec![
        Span::styled(" Profile [p]: ", Style::default().fg(Color::Gray)),
        Span::styled(pr_name, pr_style),
        Span::styled(
            if show_profile { "  <Up/Down>" } else { "" },
            Style::default().fg(if pr_focused { Color::Green } else { Color::DarkGray }),
        ),
    ]);
    let pr_block = Block::default().borders(Borders::ALL);
    frame.render_widget(Paragraph::new(pr_line).block(pr_block), vert[3]);

    // --- Action area ---
    let mut action_lines = vec![
        Line::from(Span::styled(
            " Press [v] to start video export",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
    ];

    if let Some(ref info) = app.file_info {
        action_lines.push(Line::from(Span::styled(
            format!(" Source: {}", info.path.split('/').last().unwrap_or(&info.path)),
            Style::default().fg(Color::Cyan),
        )));
        action_lines.push(Line::from(Span::styled(
            format!(" {}x{} | {} frames | {:.1}fps",
                info.width, info.height, info.frame_count, info.fps),
            Style::default().fg(Color::Gray),
        )));
        action_lines.push(Line::from(""));
    }

    if app.is_exporting {
        action_lines.push(Line::from(Span::styled(
            format!(" Progress: {:.1}%", app.export_progress),
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )));
        action_lines.push(Line::from(Span::styled(
            " Press [Ctrl+X] to cancel export",
            Style::default().fg(Color::Red),
        )));
    }

    let action_block = Paragraph::new(action_lines)
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(action_block, vert[4]);
}

fn render_right_pane(frame: &mut Frame, app: &App, area: Rect) {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(4),
        ])
        .split(area);

    let mut queue_text = vec![];
    if app.encode_jobs.is_empty() && !app.is_exporting {
        queue_text.push(Line::from(Span::styled(
            "No active jobs",
            Style::default().fg(Color::Gray),
        )));
    }
    for job in &app.encode_jobs {
        let pbar = format!("[{:.0}%]", job.progress);
        let status_str = match &job.status {
            EncodeStatus::Queued => "Queued",
            EncodeStatus::Running => "Running",
            EncodeStatus::Completed => "Completed",
            EncodeStatus::Failed(_) => "Failed",
        };
        queue_text.push(Line::from(vec![
            Span::styled(format!("  {} ", job.format_label()), Style::default().fg(Color::Green)),
            Span::styled(pbar, Style::default().fg(Color::Gray)),
            Span::raw(" "),
            Span::styled(status_str, Style::default().fg(Color::Yellow)),
        ]));
    }

    let queue = Paragraph::new(queue_text)
        .block(Block::default().title(" Queue ").borders(Borders::ALL));
    frame.render_widget(queue, vert[0]);

    if app.is_exporting {
        let gauge = Gauge::default()
            .block(Block::default().title(" Export Progress ").borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Green).bg(Color::Black))
            .percent(app.export_progress as u16)
            .label(format!("{:.1}%", app.export_progress));
        frame.render_widget(gauge, vert[1]);
    } else {
        let idle = Paragraph::new(Line::from(Span::styled(
            " Idle",
            Style::default().fg(Color::Gray),
        )))
        .block(Block::default().title(" Export Progress ").borders(Borders::ALL));
        frame.render_widget(idle, vert[1]);
    }
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    let status_text = format!(
        " {} | Screen: {:?} | Files: {} | Jobs: {}",
        app.status_message,
        app.screen,
        app.browser.entries.len(),
        app.encode_jobs.len(),
    );

    let status = Paragraph::new(Span::styled(
        status_text,
        Style::default().fg(Color::Gray),
    ))
    .block(Block::default().borders(Borders::ALL).title(" Status "));
    frame.render_widget(status, area);
}
