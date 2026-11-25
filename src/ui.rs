//! All rendering. The layout is a header, three columns (buffers, messages,
//! members), an input line and a hint line; menus float on top.

use ratatui::backend::TestBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};

use crate::app::{App, Focus, Mode, SideEntry};
use crate::list::ChannelList;
use crate::model::{Buffer, BufferKind, MsgKind};
use crate::util;

const BORDER: Color = Color::Rgb(58, 64, 82);
const ACCENT: Color = Color::Rgb(122, 178, 255);
const DIM: Color = Color::Rgb(115, 123, 143);
const TIME: Color = Color::Rgb(92, 100, 120);
const TEXT: Color = Color::Rgb(214, 220, 234);
const GREEN: Color = Color::Rgb(130, 200, 120);
const RED: Color = Color::Rgb(240, 110, 120);
const YELLOW: Color = Color::Rgb(230, 190, 100);
const MAGENTA: Color = Color::Rgb(205, 140, 235);
const CYAN: Color = Color::Rgb(115, 205, 220);
const SEL_CURSOR: Color = Color::Rgb(46, 56, 76);
const SEL_RANGE: Color = Color::Rgb(33, 41, 58);
const TOP_BG: Color = Color::Rgb(22, 26, 36);
const INPUT_BG: Color = Color::Rgb(20, 24, 33);
const ACTIVE_BG: Color = Color::Rgb(37, 45, 62);
const NAV_BG: Color = Color::Rgb(29, 35, 49);

struct Row {
    msg: usize,
    line: Line<'static>,
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    app.width = area.width;
    app.height = area.height;
    app.hit = crate::app::Hit::default();

    if let Some(list) = app.list.as_ref() {
        draw_channel_list(f, app, list, area);
        if let Some(menu) = app.menu.as_ref() {
            draw_menu(f, menu, area);
        }
        return;
    }

    if app.focus == Focus::Members && !app.members_visible() {
        app.focus = Focus::Messages;
    }
    if app.focus == Focus::Channels && !app.sidebar_visible() {
        app.focus = Focus::Messages;
    }

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(area);

    draw_header(f, app, rows[0]);
    draw_main(f, app, rows[1]);
    draw_bottom(f, app, rows[2]);

    if let Some(menu) = app.menu.as_ref() {
        draw_menu(f, menu, area);
    } else if app.mode.wants_input() {
        place_cursor(f, app, rows[2]);
    }
}

// ------------------------------------------------------------------ header ---

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let base = Style::default().bg(TOP_BG).fg(DIM);
    let cols = Layout::horizontal([Constraint::Min(20), Constraint::Length(34)]).split(area);

    let mut left = vec![
        Span::styled(" ◆ ", Style::default().fg(ACCENT).bg(TOP_BG)),
        Span::styled(
            "rirc",
            Style::default()
                .fg(Color::Rgb(235, 240, 250))
                .bg(TOP_BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", base),
    ];
    if let Some(buffer) = app.active_buffer() {
        left.push(Span::styled(
            buffer.name().to_string(),
            Style::default()
                .fg(TEXT)
                .bg(TOP_BG)
                .add_modifier(Modifier::BOLD),
        ));
        if let Some(server) = app.server(buffer.server) {
            left.push(Span::styled(
                format!("  ·  {}", server.display()),
                Style::default().fg(DIM).bg(TOP_BG),
            ));
            let dot = match server.status() {
                "connected" => " ●",
                "connecting" => " ◌",
                "local" => " ◇",
                _ => " ○",
            };
            let color = match server.status() {
                "connected" => GREEN,
                "connecting" => YELLOW,
                "local" => DIM,
                _ => RED,
            };
            left.push(Span::styled(dot, Style::default().fg(color).bg(TOP_BG)));
        }
    }
    f.render_widget(Paragraph::new(Line::from(left)).style(base), cols[0]);

    let (mode, mode_bg, mode_fg) = if app.menu.is_some() {
        ("MENU", Color::Rgb(28, 60, 70), Color::Rgb(140, 225, 235))
    } else {
        let label = app.mode.label();
        let (bg, fg) = match app.mode {
            Mode::Normal => (Color::Rgb(40, 52, 78), Color::Rgb(160, 190, 240)),
            Mode::Visual { .. } => (Color::Rgb(76, 42, 80), Color::Rgb(240, 180, 245)),
            Mode::Insert => (Color::Rgb(35, 62, 46), Color::Rgb(150, 230, 165)),
            Mode::Command => (Color::Rgb(72, 60, 30), Color::Rgb(240, 215, 140)),
        };
        (label, bg, fg)
    };
    let nick = app
        .active_buffer()
        .and_then(|b| app.server(b.server))
        .map(|s| s.nick.clone())
        .unwrap_or_default();
    let right = Line::from(vec![
        Span::styled(
            format!(" {mode} "),
            Style::default()
                .bg(mode_bg)
                .fg(mode_fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {nick} "), Style::default().fg(DIM).bg(TOP_BG)),
    ]);
    f.render_widget(
        Paragraph::new(right)
            .style(base)
            .alignment(Alignment::Right),
        cols[1],
    );
}

// -------------------------------------------------------------------- main ---

fn draw_main(f: &mut Frame, app: &mut App, area: Rect) {
    let show_side = app.sidebar_visible();
    let show_members = app.members_visible();
    let side_w = ((area.width as usize / 5).clamp(18, 26)) as u16;
    let member_w = ((area.width as usize / 6).clamp(16, 24)) as u16;

    let mut constraints = Vec::new();
    if show_side {
        constraints.push(Constraint::Length(side_w));
    }
    constraints.push(Constraint::Min(24));
    if show_members {
        constraints.push(Constraint::Length(member_w));
    }
    let cols = Layout::horizontal(constraints).split(area);
    let mut i = 0;
    if show_side {
        draw_sidebar(f, app, cols[i]);
        i += 1;
    }
    draw_messages(f, app, cols[i]);
    if show_members {
        draw_members(f, app, cols[i + 1]);
    }
}

fn panel_block(title: Line<'static>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { BORDER }))
        .title(title)
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(bid) = app.active else {
        let block = panel_block(Line::from(" messages "), false);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let hint = Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  no buffer open",
                Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  :connect irc.libera.chat",
                Style::default().fg(ACCENT),
            )),
            Line::from(Span::styled(
                "  ? for keyboard help",
                Style::default().fg(DIM),
            )),
        ]);
        f.render_widget(
            Paragraph::new(hint).alignment(Alignment::Center),
            centered(inner, 40, 5),
        );
        return;
    };

    let focus = app.focus == Focus::Messages && !app.mode.wants_input();
    let (name, topic, unread_line) = match app.buffers.get(bid) {
        Some(buffer) => {
            let topic = buffer.topic().unwrap_or("").to_string();
            let unread = if buffer.unread > 0 {
                format!("  {} new ", buffer.unread)
            } else {
                String::new()
            };
            (buffer.name().to_string(), topic, unread)
        }
        None => return,
    };

    let mut title = vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            name.clone(),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ),
    ];
    if !topic.is_empty() {
        let room = (area.width as usize)
            .saturating_sub(util::width(&name) + 10)
            .max(8);
        title.push(Span::styled(
            format!("  —  {}", util::truncate(&topic, room)),
            Style::default().fg(DIM),
        ));
    }
    if !unread_line.is_empty() {
        title.push(Span::styled(unread_line, Style::default().fg(YELLOW)));
    }
    title.push(Span::styled(" ", Style::default()));

    let block = panel_block(Line::from(title), focus);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.hit.messages_rect = inner;
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let buffer = match app.buffers.get(bid) {
        Some(buffer) => buffer,
        None => return,
    };
    let rows = build_rows(buffer, inner.width as usize);
    let view_h = inner.height as usize;
    let max_offset = rows.len().saturating_sub(view_h);
    let mut offset = max_offset.saturating_sub(buffer.scroll.min(max_offset));

    let selection = app.selection_bounds();
    let cursor = if let Some(buffer) = app.buffers.get(bid) {
        buffer.selected
    } else {
        0
    };
    let reveal = matches!(app.mode, Mode::Normal | Mode::Visual { .. });
    if reveal {
        if let Some((start, end)) = selection {
            if let Some(first) = rows.iter().position(|r| r.msg >= start) {
                if let Some(last) = rows.iter().rposition(|r| r.msg <= end) {
                    if last >= offset + view_h {
                        offset = last + 1 - view_h;
                    }
                    if first < offset {
                        offset = first;
                    }
                }
            }
        }
    }
    offset = offset.min(max_offset);

    let (sel_start, sel_end) = selection.unwrap_or((usize::MAX, usize::MAX));
    let mut items: Vec<ListItem> = Vec::new();
    let mut hit = Vec::new();
    for (row_i, row) in rows.iter().skip(offset).take(view_h).enumerate() {
        let mut line = row.line.clone();
        let selected = row.msg >= sel_start && row.msg <= sel_end;
        if selected {
            let used = line.width();
            if used < inner.width as usize {
                line.spans
                    .push(Span::raw(" ".repeat(inner.width as usize - used)));
            }
        }
        let mut item = ListItem::new(line);
        if selected {
            let bg = if row.msg == cursor {
                SEL_CURSOR
            } else {
                SEL_RANGE
            };
            item = item.style(Style::default().bg(bg));
        }
        items.push(item);
        let y = inner.y + row_i as u16;
        if hit.last().map(|(last_y, _)| *last_y) != Some(y) {
            hit.push((y, row.msg));
        }
    }
    f.render_widget(List::new(items), inner);
    app.hit.messages = hit;

    if let Some(buffer) = app.buffers.get_mut(bid) {
        buffer.scroll = max_offset.saturating_sub(offset);
    }
}

fn build_rows(buffer: &Buffer, width: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    for (idx, msg) in buffer.messages.iter().enumerate() {
        if msg.text.is_empty() && !msg.kind.is_chat() {
            rows.push(Row {
                msg: idx,
                line: Line::from(""),
            });
            continue;
        }
        let time = format!("[{}] ", util::clock(msg.ts));
        let time_span = Span::styled(time, Style::default().fg(TIME));
        match msg.kind {
            MsgKind::Privmsg | MsgKind::Action | MsgKind::Notice => {
                let nick = msg.nick.clone().unwrap_or_else(|| "?".to_string());
                let shown = util::truncate(&nick, (width / 4).clamp(8, 16));
                let color = if msg.own {
                    GREEN
                } else {
                    util::nick_color(&nick)
                };
                let nick_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
                let mut head = vec![time_span];
                match msg.kind {
                    MsgKind::Action => {
                        head.push(Span::styled("* ", Style::default().fg(MAGENTA)));
                        head.push(Span::styled(shown, nick_style));
                        head.push(Span::raw(" "));
                    }
                    MsgKind::Notice => {
                        head.push(Span::styled("-", Style::default().fg(MAGENTA)));
                        head.push(Span::styled(shown, nick_style));
                        head.push(Span::styled("- ", Style::default().fg(MAGENTA)));
                    }
                    _ => {
                        head.push(Span::styled(shown, nick_style));
                        head.push(Span::raw(" "));
                    }
                }
                let text_style = if msg.highlight {
                    Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)
                } else if msg.kind == MsgKind::Notice {
                    Style::default().fg(Color::Rgb(196, 176, 220))
                } else {
                    Style::default().fg(TEXT)
                };
                push_message_rows(&mut rows, idx, head, &msg.text, width, text_style);
            }
            kind => {
                let (icon, color) = match kind {
                    MsgKind::Join => ("→ ", GREEN),
                    MsgKind::Part => ("← ", YELLOW),
                    MsgKind::Quit => ("× ", RED),
                    MsgKind::Kick => ("! ", RED),
                    MsgKind::Mode => ("⚙ ", DIM),
                    MsgKind::Topic => ("§ ", CYAN),
                    MsgKind::Nick => ("~ ", MAGENTA),
                    MsgKind::Error => ("✗ ", RED),
                    MsgKind::Motd => ("│ ", DIM),
                    _ => ("· ", DIM),
                };
                let head = vec![time_span, Span::styled(icon, Style::default().fg(color))];
                let text_style = if kind == MsgKind::Error {
                    Style::default().fg(RED)
                } else if kind == MsgKind::Info && msg.highlight {
                    Style::default().fg(YELLOW)
                } else {
                    Style::default().fg(color)
                };
                push_message_rows(&mut rows, idx, head, &msg.text, width, text_style);
            }
        }
    }
    if rows.is_empty() {
        rows.push(Row {
            msg: 0,
            line: Line::from(Span::styled(
                "no messages yet",
                Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
            )),
        });
    }
    rows
}

fn push_message_rows(
    out: &mut Vec<Row>,
    idx: usize,
    head: Vec<Span<'static>>,
    text: &str,
    width: usize,
    style: Style,
) {
    let head_width: usize = head.iter().map(|s| util::width(&s.content)).sum();
    let body_width = width.saturating_sub(head_width).max(6);
    let lines = util::wrap_widths(text, body_width, body_width);
    for (i, chunk) in lines.iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if i == 0 {
            spans.extend(head.clone());
        } else {
            spans.push(Span::raw(" ".repeat(head_width)));
        }
        spans.push(Span::styled(chunk.clone(), style));
        out.push(Row {
            msg: idx,
            line: Line::from(spans),
        });
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w,
        h,
    )
}

// ------------------------------------------------------------------ sidebar ---

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Channels && !app.mode.wants_input();
    let block = panel_block(Line::from(" buffers "), focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 4 || inner.height == 0 {
        return;
    }
    let entries = app.sidebar_entries();
    let width = inner.width as usize;
    let height = inner.height as usize;
    let nav = app.sidebar_nav.min(entries.len().saturating_sub(1));
    app.sidebar_nav = nav;
    let offset = if nav >= height { nav + 1 - height } else { 0 };
    let mut lines: Vec<Line> = Vec::new();
    let mut hit = Vec::new();

    for (i, entry) in entries.iter().enumerate().skip(offset).take(height) {
        let y = inner.y + (i - offset) as u16;
        hit.push((y, i));
        let is_nav = focused && i == nav;
        match *entry {
            SideEntry::Server(pos) => {
                let Some(server) = app.servers.get(pos) else {
                    continue;
                };
                let active = app
                    .active
                    .and_then(|a| app.buffers.get(a))
                    .map(|b| b.server == server.id && b.is_server())
                    .unwrap_or(false);
                let mut base = Style::default();
                if active {
                    base = base.bg(ACTIVE_BG);
                } else if is_nav {
                    base = base.bg(NAV_BG);
                }
                let (dot, dot_color) = match server.status() {
                    "connected" => ("●", GREEN),
                    "connecting" => ("◌", YELLOW),
                    "local" => ("◇", DIM),
                    _ => ("○", RED),
                };
                let arrow = if app
                    .buffers
                    .iter()
                    .any(|b| b.server == server.id && !b.is_server())
                {
                    "▾"
                } else {
                    "▸"
                };
                let label = util::truncate(server.display(), width.saturating_sub(4).max(4));
                let mut line = Line::from(vec![
                    Span::styled(format!("{arrow} "), Style::default().fg(DIM)),
                    Span::styled(format!("{dot} "), Style::default().fg(dot_color)),
                    Span::styled(
                        label,
                        Style::default()
                            .fg(if active { Color::White } else { TEXT })
                            .add_modifier(Modifier::BOLD),
                    ),
                ]);
                pad_line(&mut line, width, base);
                lines.push(line.style(base));
            }
            SideEntry::Buffer(bid) => {
                let Some(buffer) = app.buffers.get(bid) else {
                    continue;
                };
                let active = app.active == Some(bid);
                let unread = buffer.unread;
                let mentions = buffer.mentions;
                let name = buffer.name().to_string();
                let is_channel = buffer.is_channel();
                let joined = matches!(&buffer.kind, BufferKind::Channel { joined: true, .. });
                let mut base = Style::default();
                if active {
                    base = base.bg(ACTIVE_BG).add_modifier(Modifier::BOLD);
                } else if is_nav {
                    base = base.bg(NAV_BG);
                }
                let badge = if mentions > 0 {
                    format!("@{mentions}")
                } else if unread > 0 {
                    unread.to_string()
                } else {
                    String::new()
                };
                let badge_style = if mentions > 0 {
                    Style::default().fg(YELLOW).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(DIM)
                };
                let badge_len = badge.chars().count();
                let name_room = width.saturating_sub(3).saturating_sub(if badge_len > 0 {
                    badge_len + 1
                } else {
                    0
                });
                let shown = util::truncate(&name, name_room.max(4));
                let name_color = if active {
                    Color::White
                } else if is_channel && !joined {
                    DIM
                } else if is_channel {
                    Color::Rgb(150, 190, 225)
                } else {
                    Color::Rgb(205, 155, 225)
                };
                let mut spans = vec![
                    Span::raw("  "),
                    Span::styled(shown, Style::default().fg(name_color)),
                ];
                let used: usize = spans.iter().map(|s| util::width(&s.content)).sum();
                let gap = width.saturating_sub(used + badge_len);
                if gap > 0 {
                    spans.push(Span::raw(" ".repeat(gap)));
                }
                if !badge.is_empty() {
                    spans.push(Span::styled(badge, badge_style));
                }
                let line = Line::from(spans).style(base);
                lines.push(line);
            }
        }
    }

    let paragraph = Paragraph::new(Text::from(lines));
    f.render_widget(paragraph, inner);
    app.hit.sidebar = hit;
    app.hit.sidebar_rect = inner;
}

fn pad_line(line: &mut Line<'static>, width: usize, style: Style) {
    let used = line.width();
    if used < width {
        line.spans
            .push(Span::styled(" ".repeat(width - used), style));
    }
}

// ------------------------------------------------------------------ members ---

fn draw_members(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Members && !app.mode.wants_input();
    let Some(bid) = app.active else {
        let block = panel_block(Line::from(" members "), focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.width > 0 && inner.height > 0 {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "no buffer",
                    Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
                )),
                inner,
            );
        }
        return;
    };
    let count = app
        .buffers
        .get(bid)
        .map(|buffer| match &buffer.kind {
            BufferKind::Channel { members, .. } => members.len(),
            BufferKind::Query { .. } => 1,
            BufferKind::Server { .. } => 0,
        })
        .unwrap_or(0);
    let nav = app.member_nav.min(count.saturating_sub(1));
    app.member_nav = nav;
    let Some(buffer) = app.buffers.get(bid) else {
        return;
    };
    let sid = buffer.server;
    let my_nick = app.nick_of(sid);
    let server = app.server(sid);
    let order = app.member_order(bid);
    let title = match &buffer.kind {
        BufferKind::Channel { members, .. } => format!(" members {} ", members.len()),
        BufferKind::Query { member } => format!(" {} ", util::truncate(&member.nick, 18)),
        BufferKind::Server { .. } => " members ".to_string(),
    };
    let block = panel_block(
        Line::from(vec![Span::styled(title, Style::default().fg(DIM))]),
        focused,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 4 || inner.height == 0 {
        return;
    }
    let width = inner.width as usize;
    let height = inner.height as usize;
    let offset = if nav >= height { nav + 1 - height } else { 0 };
    let mut lines = Vec::new();
    let mut hit = Vec::new();

    match &buffer.kind {
        BufferKind::Channel { .. } => {
            for (row, &member_idx) in order.iter().enumerate().skip(offset).take(height) {
                let Some(member) = buffer.members().get(member_idx) else {
                    continue;
                };
                let mut base = Style::default();
                let is_nav = focused && row == nav;
                if is_nav {
                    base = base.bg(NAV_BG);
                }
                let mut line = member_line(member, server, &my_nick, width).style(base);
                pad_line(&mut line, width, base);
                lines.push(line);
                let y = inner.y + (row - offset) as u16;
                hit.push((y, row));
            }
        }
        BufferKind::Query { member } => {
            let mut base = Style::default();
            if focused {
                base = base.bg(NAV_BG);
            }
            let mut line = member_line(member, server, &my_nick, width).style(base);
            pad_line(&mut line, width, base);
            lines.push(line);
            hit.push((inner.y, 0));
        }
        BufferKind::Server { .. } => {
            lines.push(Line::from(Span::styled(
                "no channel selected",
                Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
            )));
        }
    }

    f.render_widget(Paragraph::new(Text::from(lines)), inner);
    app.hit.members = hit;
    app.hit.members_rect = inner;
}

fn member_line(
    member: &crate::model::Member,
    server: Option<&crate::model::Server>,
    my_nick: &str,
    width: usize,
) -> Line<'static> {
    let is_me = util::fold(&member.nick) == util::fold(my_nick);
    let mut spans = Vec::new();
    let mut prefixes: Vec<char> = member
        .prefixes
        .chars()
        .filter(|c| {
            server
                .map(|s| s.mode_for_prefix(*c).is_some())
                .unwrap_or(false)
        })
        .collect();
    prefixes.sort_by_key(|p| server.map(|s| s.rank_for(&p.to_string())).unwrap_or(0));
    for p in prefixes {
        let color = match server.and_then(|s| s.mode_for_prefix(p)) {
            Some('q' | 'a') => MAGENTA,
            Some('o') => YELLOW,
            Some('h') => CYAN,
            Some('v') => GREEN,
            _ => DIM,
        };
        spans.push(Span::styled(p.to_string(), Style::default().fg(color)));
    }
    let nick_color = if is_me {
        GREEN
    } else {
        util::nick_color(&member.nick)
    };
    let nick_style = Style::default().fg(nick_color).add_modifier(if is_me {
        Modifier::BOLD | Modifier::UNDERLINED
    } else {
        Modifier::empty()
    });
    spans.push(Span::styled(
        util::truncate(&member.nick, width.saturating_sub(5)),
        nick_style,
    ));
    if member.away.is_some() {
        spans.push(Span::styled(" away", Style::default().fg(DIM)));
    }
    Line::from(spans)
}

// ------------------------------------------------------------------- bottom ---

fn draw_bottom(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    draw_input(f, app, chunks[0]);
    draw_hints(f, app, chunks[1]);
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let base = Style::default().bg(INPUT_BG);
    let prompt = match app.mode {
        Mode::Insert => Span::styled(" > ", Style::default().fg(GREEN).bg(INPUT_BG)),
        Mode::Command => Span::styled(" > ", Style::default().fg(YELLOW).bg(INPUT_BG)),
        _ => Span::styled("   ", Style::default().bg(INPUT_BG)),
    };
    let mut spans = vec![prompt];
    let full = app.input.text();
    if full.is_empty() {
        let placeholder = if app.mode.wants_input() {
            ""
        } else {
            "press i to write…"
        };
        if !placeholder.is_empty() {
            spans.push(Span::styled(
                placeholder,
                Style::default()
                    .fg(DIM)
                    .bg(INPUT_BG)
                    .add_modifier(Modifier::ITALIC),
            ));
        }
    } else {
        let skip = input_skip(app, (area.width as usize).saturating_sub(3));
        let text: String = app.input.chars.iter().skip(skip).collect();
        let style = if app.mode.wants_input() {
            Style::default().fg(TEXT).bg(INPUT_BG)
        } else {
            Style::default().fg(DIM).bg(INPUT_BG)
        };
        spans.push(Span::styled(text, style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(base), area);
}

fn input_skip(app: &App, limit: usize) -> usize {
    if limit == 0 {
        return 0;
    }
    let cursor = app.input.cursor.min(app.input.chars.len());
    let mut skip = 0;
    let mut before: usize = app.input.chars[..cursor]
        .iter()
        .map(|c| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0))
        .sum();
    while skip < cursor && before >= limit {
        let width = unicode_width::UnicodeWidthChar::width(app.input.chars[skip]).unwrap_or(0);
        before = before.saturating_sub(width);
        skip += 1;
    }
    skip
}

fn place_cursor(f: &mut Frame, app: &App, bottom: Rect) {
    let area = Rect::new(bottom.x, bottom.y, bottom.width, 1);
    let skip = input_skip(app, (area.width as usize).saturating_sub(3));
    let before: String = app
        .input
        .chars
        .iter()
        .skip(skip)
        .take(app.input.cursor.saturating_sub(skip))
        .collect();
    let x = area.x + 3 + util::width(&before) as u16;
    let x = x.min(area.x.saturating_add(area.width).saturating_sub(1));
    f.set_cursor_position(Position::new(x, area.y));
}

fn draw_hints(f: &mut Frame, app: &App, area: Rect) {
    let hint: String = match app.mode {
        Mode::Visual { .. } => {
            let selected = app
                .selection_bounds()
                .map(|(start, end)| end - start + 1)
                .unwrap_or(0);
            format!(
                "VISUAL {selected} selected · j/k extend · o swap · y yank · Enter menu · Esc cancel"
            )
        }
        Mode::Insert => "TEXT · Enter send · Tab complete · ↑/↓ history · Esc normal".to_string(),
        Mode::Command => "COMMAND · Enter run · Tab complete · Esc cancel · :q quit".to_string(),
        Mode::Normal => match app.focus {
            Focus::Channels => {
                "NORMAL · j/k pick · Enter menu · l open · i write · Tab panel".to_string()
            }
            Focus::Members => {
                let channel = app
                    .active_buffer()
                    .map(|buffer| buffer.is_channel())
                    .unwrap_or(false);
                if channel {
                    "NORMAL · j/k pick · Enter menu · l message · y copy nick".to_string()
                } else {
                    "NORMAL · Enter menu · Tab panel · i write · ? help".to_string()
                }
            }
            Focus::Messages => {
                "NORMAL · j/k move · Enter menu · v select · y yank · i write · Tab panel"
                    .to_string()
            }
        },
    };
    let mut spans = vec![Span::styled(format!("  {hint}"), Style::default().fg(DIM))];
    if let Some((text, _)) = &app.toast {
        spans.push(Span::styled(
            format!("   ·   {text}"),
            Style::default().fg(YELLOW),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ----------------------------------------------------------------- list ---

fn draw_channel_list(f: &mut Frame, app: &App, list: &ChannelList, area: Rect) {
    if area.width < 24 || area.height < 8 {
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(Span::styled(
                "list: too small · Esc closes",
                Style::default().fg(DIM),
            )),
            area,
        );
        return;
    }
    let rect = area;
    f.render_widget(Clear, rect);

    let filtered = list.filtered();
    let status = if list.loading {
        format!("{} so far…", list.entries.len())
    } else if list.error.is_some() {
        "error".to_string()
    } else {
        format!("{} channels", list.entries.len())
    };
    let mut title_spans = vec![
        Span::raw(" "),
        Span::styled(
            "channel list",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ·  {}", list.server_name),
            Style::default().fg(DIM),
        ),
        Span::styled(
            format!("  ·  {status}"),
            Style::default().fg(if list.loading { YELLOW } else { DIM }),
        ),
    ];
    if !list.filter.trim().is_empty() {
        title_spans.push(Span::styled(
            format!("  ·  {} shown", filtered.len()),
            Style::default().fg(CYAN),
        ));
    }
    title_spans.push(Span::raw(" "));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(ACCENT))
        .title(Line::from(title_spans));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.width < 10 || inner.height < 4 {
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let inner_w = inner.width as usize;

    // Filter line.
    let mut filter_spans = vec![Span::styled(
        " filter  ",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )];
    if list.filter.is_empty() && !list.filtering {
        filter_spans.push(Span::styled(
            "(none)  ·  f or / to filter",
            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
        ));
    } else {
        let text = if list.filter.is_empty() {
            "type to filter…".to_string()
        } else {
            list.filter.clone()
        };
        filter_spans.push(Span::styled(
            text,
            Style::default().fg(if list.filtering { Color::White } else { CYAN }),
        ));
        if list.filtering {
            filter_spans.push(Span::styled("▌", Style::default().fg(ACCENT)));
        }
        if !list.filter.trim().is_empty() {
            filter_spans.push(Span::styled(
                format!("   {} shown", filtered.len()),
                Style::default().fg(DIM),
            ));
        }
    }
    f.render_widget(Paragraph::new(Line::from(filter_spans)), chunks[0]);

    // Column layout.
    let marker_w = 2usize;
    let users_w = 6usize;
    let name_room = inner_w.saturating_sub(marker_w + users_w + 1);
    let name_w = (name_room / 2).clamp(10, 30).min(name_room.max(1));
    let topic_w = inner_w.saturating_sub(marker_w + name_w + users_w + 1);

    let mut header = String::from("  ");
    header.push_str(&format!("{:<width$}", "#channel", width = name_w));
    header.push_str(&format!("{:>width$} ", "users", width = users_w));
    header.push_str(&util::truncate("topic", topic_w));
    f.render_widget(
        Paragraph::new(Span::styled(
            header,
            Style::default().fg(DIM).add_modifier(Modifier::UNDERLINED),
        )),
        chunks[1],
    );

    // Rows.
    let rows_height = chunks[2].height as usize;
    let selected_pos = filtered
        .iter()
        .position(|&idx| idx == list.selected)
        .unwrap_or(0);
    let offset = if selected_pos >= rows_height {
        selected_pos + 1 - rows_height
    } else {
        0
    };
    let mut lines: Vec<Line> = Vec::new();
    if filtered.is_empty() {
        let message = if list.loading {
            "loading channel list…".to_string()
        } else if !list.filter.trim().is_empty() {
            "no channels match the filter".to_string()
        } else if let Some(error) = &list.error {
            format!("LIST failed: {error}")
        } else {
            "no channels returned".to_string()
        };
        lines.push(Line::from(Span::styled(
            format!("  {message}"),
            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
        )));
    } else {
        for &idx in filtered.iter().skip(offset).take(rows_height) {
            let entry = &list.entries[idx];
            let selected = idx == list.selected;
            let joined = app.find_channel(list.server, &entry.channel).is_some();
            let marker = if selected {
                "▸ "
            } else if joined {
                "• "
            } else {
                "  "
            };
            let marker_color = if selected {
                ACCENT
            } else if joined {
                GREEN
            } else {
                DIM
            };
            let name_text = util::truncate(&entry.channel, name_w);
            let name_pad = name_w.saturating_sub(util::width(&name_text));
            let users = format!("{:>width$}", entry.users, width = users_w);
            let topic = util::truncate(&entry.topic, topic_w);
            let name_style = Style::default()
                .fg(if selected { Color::White } else { ACCENT })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });
            let spans = vec![
                Span::styled(marker, Style::default().fg(marker_color)),
                Span::styled(name_text, name_style),
                Span::raw(" ".repeat(name_pad)),
                Span::styled(
                    users,
                    Style::default().fg(if selected { TEXT } else { DIM }),
                ),
                Span::raw(" "),
                Span::styled(
                    topic,
                    Style::default().fg(if selected { TEXT } else { DIM }),
                ),
            ];
            let base = if selected {
                Style::default().bg(SEL_CURSOR)
            } else {
                Style::default()
            };
            let mut line = Line::from(spans);
            pad_line(&mut line, inner_w, base);
            lines.push(line.style(base));
        }
    }
    f.render_widget(Paragraph::new(Text::from(lines)), chunks[2]);

    // Selected channel detail.
    let detail_entry = filtered
        .iter()
        .position(|&idx| idx == list.selected)
        .map(|position| &list.entries[filtered[position]])
        .or_else(|| filtered.first().map(|&idx| &list.entries[idx]));
    let detail = if let Some(error) = &list.error {
        Line::from(vec![
            Span::styled(" error  ", Style::default().fg(RED)),
            Span::styled(error.clone(), Style::default().fg(RED)),
        ])
    } else if let Some(entry) = detail_entry {
        let joined = app.find_channel(list.server, &entry.channel).is_some();
        let mut text = format!(" {} · {} users", entry.channel, entry.users);
        if joined {
            text.push_str(" · joined");
        }
        if !entry.topic.is_empty() {
            text.push_str(" · ");
            text.push_str(&entry.topic);
        }
        Line::from(Span::styled(
            util::truncate(&text, inner_w),
            Style::default().fg(TEXT),
        ))
    } else if list.loading {
        Line::from(Span::styled(
            format!(" {} channels received…", list.entries.len()),
            Style::default().fg(DIM),
        ))
    } else if !list.filter.trim().is_empty() {
        Line::from(Span::styled(
            " no channel matches the filter",
            Style::default().fg(DIM),
        ))
    } else {
        Line::from("")
    };
    f.render_widget(Paragraph::new(detail), chunks[3]);

    let hint = if list.filtering {
        " type to filter · Enter apply · Esc finish · Ctrl-u clear"
    } else {
        " j/k move · Enter menu · l join · f filter · r refresh · Esc close"
    };
    let mut hint_spans = vec![Span::styled(hint, Style::default().fg(DIM))];
    if let Some((text, _)) = &app.toast {
        hint_spans.push(Span::styled(
            format!("   ·   {text}"),
            Style::default().fg(YELLOW),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(hint_spans)), chunks[4]);
}

// --------------------------------------------------------------------- menu ---

fn draw_menu(f: &mut Frame, menu: &crate::menu::Menu, area: Rect) {
    if area.width < 6 || area.height < 4 {
        return;
    }
    let max_w = area.width.saturating_sub(4).clamp(30, 80).min(area.width);
    let width = max_w.min(area.width);
    let inner_width = width.saturating_sub(4) as usize;
    let mut info_lines: Vec<Line> = Vec::new();
    for (i, raw) in menu.info.iter().enumerate() {
        let style = if i == 0 {
            Style::default().fg(TEXT)
        } else {
            Style::default().fg(DIM)
        };
        if raw.is_empty() {
            info_lines.push(Line::from(""));
            continue;
        }
        for chunk in util::wrap(raw, inner_width) {
            info_lines.push(Line::from(Span::styled(chunk, style)));
        }
    }
    let action_count = menu.actions.len();
    let wanted = info_lines.len() as u16 + action_count as u16 + 4;
    let height = wanted
        .min(area.height.saturating_sub(2))
        .max(4)
        .min(area.height);
    let rect = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );

    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(ACCENT))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                menu.title.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let hint_h = 1u16;
    let avail = inner.height;
    let action_h = (action_count as u16).min(avail.saturating_sub(hint_h + 1));
    let info_h = avail
        .saturating_sub(action_h + hint_h)
        .saturating_sub(if action_h > 0 { 1 } else { 0 });

    let info_rect = Rect::new(inner.x, inner.y, inner.width, info_h);
    f.render_widget(Paragraph::new(Text::from(info_lines)), info_rect);

    if action_h > 0 {
        let sep_y = inner.y + info_h;
        let sep = Line::from(Span::styled(
            "─".repeat(inner.width as usize),
            Style::default().fg(BORDER),
        ));
        f.render_widget(
            Paragraph::new(sep),
            Rect::new(inner.x, sep_y, inner.width, 1),
        );
        let list_y = sep_y + 1;
        let offset = menu.selected.saturating_sub(action_h as usize - 1);
        let mut lines: Vec<Line> = Vec::new();
        for (i, action) in menu
            .actions
            .iter()
            .enumerate()
            .skip(offset)
            .take(action_h as usize)
        {
            let selected = i == menu.selected;
            let marker = if selected { "▸" } else { " " };
            let spans = vec![
                Span::styled(
                    format!(" {marker} "),
                    Style::default().fg(if selected { ACCENT } else { DIM }),
                ),
                Span::styled(format!("{}. ", i + 1), Style::default().fg(DIM)),
                Span::styled(
                    action.label(),
                    Style::default().fg(if selected { Color::White } else { TEXT }),
                ),
            ];
            let mut line = Line::from(spans);
            let base = if selected {
                Style::default().bg(SEL_CURSOR)
            } else {
                Style::default()
            };
            pad_line(&mut line, inner.width as usize, base);
            lines.push(line.style(base));
        }
        f.render_widget(
            Paragraph::new(Text::from(lines)),
            Rect::new(inner.x, list_y, inner.width, action_h),
        );
    }
    let hint = Line::from(Span::styled(
        if action_count == 0 {
            " Esc close · y copies this help"
        } else {
            " j/k move · Enter select · y copy info · Esc close"
        },
        Style::default().fg(DIM),
    ));
    f.render_widget(
        Paragraph::new(hint),
        Rect::new(
            inner.x,
            inner.y + inner.height - hint_h,
            inner.width,
            hint_h,
        ),
    );
}

// ------------------------------------------------------------------- helpers ---

pub fn render_to_string(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| draw(frame, app))
        .expect("draw snapshot");
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            line.push_str(buffer[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::model::BufferKind;
    use tokio::sync::mpsc::channel;

    #[test]
    fn demo_renders_at_many_sizes() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tx, _rx) = channel(64);
        let cfg = Config {
            demo: true,
            ..Default::default()
        };
        let mut app = App::new(cfg, Some(rt.handle().clone()), tx);
        for (w, h) in [
            (120, 38),
            (100, 30),
            (90, 24),
            (76, 12),
            (72, 20),
            (64, 10),
            (40, 15),
            (20, 8),
        ] {
            let screen = render_to_string(&mut app, w, h);
            assert_eq!(screen.lines().count(), h as usize);
        }
        let screen = render_to_string(&mut app, 120, 38);
        assert!(screen.contains("#rust"), "screen missing channel name");
        assert!(screen.contains("members"), "screen missing member panel");
        assert!(screen.contains("NORMAL"), "screen missing mode badge");
        assert!(screen.contains("buffers"), "screen missing buffer panel");

        // Visual selection must paint the row background.
        let bid = app.active.unwrap();
        let len = app.buffers[bid].messages.len();
        app.mode = Mode::Visual {
            anchor: len.saturating_sub(3),
        };
        app.buffers[bid].selected = len.saturating_sub(1);
        let backend = TestBackend::new(120, 38);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut found = false;
        for y in 0..38u16 {
            let row: String = (0..120u16)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect();
            if let Some(pos) = row.find("TUI so far") {
                assert_eq!(
                    buffer[(pos as u16, y)].bg,
                    SEL_CURSOR,
                    "selected line should have the cursor background"
                );
                found = true;
            }
        }
        assert!(found, "could not find the selected message on screen");
        app.mode = Mode::Normal;

        // Every buffer type must be renderable without panicking.
        for bid in 0..app.buffers.len() {
            app.active = Some(bid);
            app.mode = Mode::Normal;
            if let BufferKind::Channel { .. } = app.buffers[bid].kind {
                app.focus = Focus::Members;
                app.open_menu();
                app.menu = None;
            }
            render_to_string(&mut app, 100, 30);
        }
    }

    #[test]
    fn channel_list_renders_and_filters() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tx, _rx) = channel(64);
        let cfg = Config {
            demo: true,
            ..Default::default()
        };
        let mut app = App::new(cfg, Some(rt.handle().clone()), tx);
        app.run_command("list");

        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("channel list"));
        assert!(screen.contains("#rust"));
        assert!(screen.contains("users"));
        assert!(screen.contains("filter"));

        if let Some(list) = app.list.as_mut() {
            list.filter = "rust".to_string();
            list.normalize_selection();
        }
        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("shown"));
        assert!(screen.contains("#rust-beginners"));

        for (w, h) in [(80u16, 24u16), (40, 15), (24, 8)] {
            render_to_string(&mut app, w, h);
        }

        // A list open on a tiny terminal shows a friendly message.
        let screen = render_to_string(&mut app, 30, 6);
        assert!(screen.contains("too small"));

        // Closing the list restores the normal view.
        app.list = None;
        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("#rust"));
        assert!(screen.contains("buffers"));
        assert!(!screen.contains("l join"));
    }

    #[test]
    fn welcome_screen_renders_without_a_server() {
        let (tx, _rx) = channel(64);
        let cfg = Config::default();
        let mut app = App::new(cfg, None, tx);
        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("welcome"));
        assert!(screen.contains(":connect"));
        assert!(screen.contains("NORMAL"));
        app.mode = Mode::Insert;
        app.input.set_text("hello");
        render_to_string(&mut app, 100, 30);
    }

    #[test]
    fn popups_and_modes_render_their_content() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tx, _rx) = channel(64);
        let cfg = Config {
            demo: true,
            ..Default::default()
        };
        let mut app = App::new(cfg, Some(rt.handle().clone()), tx);

        app.focus = Focus::Messages;
        app.open_menu();
        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("Copy message"));
        assert!(screen.contains("Reply / mention"));
        assert!(screen.contains("MENU"));

        app.menu = None;
        app.open_help();
        let screen = render_to_string(&mut app, 100, 30);
        assert!(screen.contains("keyboard help"));
        assert!(screen.contains("VISUAL"));

        app.menu = None;
        app.mode = Mode::Insert;
        app.input
            .set_text("hello there, this is a long-ish draft message");
        let screen = render_to_string(&mut app, 60, 24);
        assert!(screen.contains("draft message"));
        assert!(screen.contains("TEXT"));

        // Popups and tiny terminals must never panic.
        app.mode = Mode::Normal;
        app.open_help();
        for (w, h) in [(30u16, 10u16), (20, 6), (10, 4)] {
            render_to_string(&mut app, w, h);
        }
    }
}
