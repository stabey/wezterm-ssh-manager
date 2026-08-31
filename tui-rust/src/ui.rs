use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Position, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Clear, Padding, Paragraph, Row, Table, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{App, EntryKind, Modal, SftpPanelView, SftpSide, TextInput},
    model::{ALL_GROUPS, profile_target},
    types::{MainPage, ManagerFocus, ProfileDraft},
};

const BG: Color = Color::Rgb(0x11, 0x11, 0x1b);
const SURFACE: Color = Color::Rgb(0x18, 0x18, 0x25);
const ELEVATED: Color = Color::Rgb(0x1e, 0x1e, 0x2e);
const OVERLAY: Color = Color::Rgb(0x31, 0x32, 0x44);
const BORDER: Color = Color::Rgb(0x45, 0x47, 0x5a);
const MUTED: Color = Color::Rgb(0x7f, 0x84, 0x9c);
const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
const SUBTEXT: Color = Color::Rgb(0xa6, 0xad, 0xc8);
const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);
const LAVENDER: Color = Color::Rgb(0xb4, 0xbe, 0xfe);
const GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
const YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
const RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);
const SELECTED: Color = Color::Rgb(0x31, 0x32, 0x44);

#[derive(Clone, Copy, Debug)]
pub(crate) struct ManagerRegions {
    pub groups: Rect,
    pub hosts: Rect,
    pub details: Option<Rect>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SftpRegions {
    pub local: Rect,
    pub remote: Rect,
}

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    let outer = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(if app.page == MainPage::Manager { 2 } else { 0 }),
    ])
    .split(area);
    draw_header(frame, outer[0], app);
    match app.page {
        MainPage::Manager => {
            draw_manager(frame, outer[1], app);
            draw_manager_footer(frame, outer[2]);
        }
        MainPage::Sftp => draw_sftp(frame, outer[1], app),
    }
    if let Some(modal) = app.modal.as_ref() {
        draw_modal(frame, area, modal);
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    frame.render_widget(Block::default().style(Style::default().bg(SURFACE)), area);
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let left = Line::from(vec![
        Span::styled("◆ SSH Manager", Style::default().fg(BLUE).bold()),
        Span::styled(
            "  [1] 主机",
            Style::default().fg(if app.page == MainPage::Manager {
                TEXT
            } else {
                MUTED
            }),
        ),
        Span::styled(
            "  [2] SFTP",
            Style::default().fg(if app.page == MainPage::Sftp {
                TEXT
            } else {
                MUTED
            }),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(left).style(Style::default().bg(SURFACE)),
        inner,
    );
    let left_width = UnicodeWidthStr::width("◆ SSH Manager  [1] 主机  [2] SFTP") as u16;
    if inner.width > left_width.saturating_add(1) {
        let status_area = Rect::new(
            inner.x.saturating_add(left_width).saturating_add(1),
            inner.y,
            inner.width.saturating_sub(left_width).saturating_sub(1),
            inner.height,
        );
        frame.render_widget(
            Paragraph::new(app.status.as_str())
                .alignment(Alignment::Right)
                .style(Style::default().fg(MUTED).bg(SURFACE)),
            status_area,
        );
    }
}

fn draw_manager(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let regions = manager_regions_from_area(area, app.show_details());
    draw_groups(frame, regions.groups, app);
    draw_hosts(frame, regions.hosts, app);
    if let Some(details) = regions.details {
        draw_details(frame, details, app);
    }
}

fn draw_groups(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let active = app.focus == ManagerFocus::Groups;
    let block = panel_block("分组", active);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = app.groups().into_iter().enumerate().map(|(index, group)| {
        let selected = index == app.group_index;
        Row::new([
            Cell::from(format!(
                "{} {}",
                if selected { "›" } else { " " },
                group.label
            )),
            Cell::from(format!("{:>3}", group.count)),
        ])
        .style(
            Style::default()
                .fg(if selected { BLUE } else { TEXT })
                .bg(if selected { SELECTED } else { SURFACE }),
        )
    });
    let table = Table::new(rows, [Constraint::Min(3), Constraint::Length(3)])
        .column_spacing(1)
        .style(Style::default().bg(SURFACE));
    frame.render_widget(table, inner);
}

fn draw_hosts(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let title = format!(
        "主机 · {}{}",
        app.hosts().len(),
        if app.filter.is_empty() {
            String::new()
        } else {
            format!(" · “{}”", app.filter)
        }
    );
    let active = app.focus == ManagerFocus::Hosts;
    let block = panel_block(title, active);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let all_groups = app.selected_group() == ALL_GROUPS;
    let rows = app
        .visible_host_window()
        .into_iter()
        .map(|(index, profile)| {
            let selected = index == app.host_index;
            let group_prefix = if all_groups && !profile.group.is_empty() {
                format!("{}/", profile.group)
            } else {
                String::new()
            };
            let icon = if profile.icon.is_empty() {
                String::new()
            } else {
                format!("{} ", profile.icon)
            };
            let status = if profile.editable {
                if profile.jump_host.is_empty() {
                    ""
                } else {
                    "跳板"
                }
            } else {
                "只读"
            };
            Row::new([
                Cell::from(format!(
                    "{} {group_prefix}{icon}{}",
                    if selected { "›" } else { " " },
                    profile.name
                )),
                Cell::from(profile_target(profile)).style(Style::default().fg(SUBTEXT)),
                Cell::from(status).style(Style::default().fg(if profile.editable {
                    GREEN
                } else {
                    YELLOW
                })),
            ])
            .style(
                Style::default()
                    .fg(if selected { BLUE } else { TEXT })
                    .bg(if selected { SELECTED } else { SURFACE }),
            )
        });
    let header = Row::new(["名称", "目标", "状态"])
        .style(Style::default().fg(MUTED).bg(ELEVATED))
        .height(1);
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(45),
            Constraint::Percentage(42),
            Constraint::Min(4),
        ],
    )
    .header(header)
    .column_spacing(0)
    .style(Style::default().bg(SURFACE));
    frame.render_widget(table, inner);

    if app.hosts().is_empty() && inner.height > 1 {
        let empty = Rect {
            y: inner.y + 1,
            height: 1,
            ..inner
        };
        frame.render_widget(
            Paragraph::new(" 没有匹配的主机，按 / 修改过滤条件")
                .style(Style::default().fg(MUTED).bg(SURFACE)),
            empty,
        );
    }
}

fn draw_details(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let block =
        panel_block("详情", app.focus == ManagerFocus::Details).padding(Padding::uniform(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(profile) = app.selected_host() else {
        frame.render_widget(
            Paragraph::new("未选择主机").style(Style::default().fg(MUTED).bg(SURFACE)),
            inner,
        );
        return;
    };
    let lines = vec![
        Line::styled(profile.name.as_str(), Style::default().fg(BLUE).bold()),
        Line::styled(profile_target(profile), Style::default().fg(TEXT)),
        Line::styled(
            if profile.group.is_empty() {
                "未分组".to_owned()
            } else {
                format!("分组  {}", profile.group)
            },
            Style::default().fg(MUTED),
        ),
        Line::styled(
            if profile.auth.is_empty() {
                "认证  自动".to_owned()
            } else {
                format!("认证  {}", profile.auth)
            },
            Style::default().fg(MUTED),
        ),
        Line::styled(
            if profile.jump_host.is_empty() {
                "跳板  无".to_owned()
            } else {
                format!("跳板  {}", profile.jump_host)
            },
            Style::default().fg(MUTED),
        ),
        Line::styled(
            if profile.editable {
                "可编辑配置"
            } else {
                "导入的只读配置"
            },
            Style::default().fg(if profile.editable { GREEN } else { YELLOW }),
        ),
        Line::raw(""),
        Line::styled("Enter 连接", Style::default().fg(SUBTEXT)),
        Line::styled("e 编辑 · s SFTP", Style::default().fg(SUBTEXT)),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(SURFACE)),
        inner,
    );
}

fn draw_manager_footer(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Block::default().style(Style::default().bg(SURFACE)), area);
    let line = "Tab 切区  ↑↓/jk 选择  Enter 连接  n 新建  e 编辑  d 删除  / 过滤  p 快连  r 刷新  s SFTP  q 返回";
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(MUTED).bg(SURFACE)),
        area.inner(Margin {
            horizontal: 1,
            vertical: 0,
        }),
    );
}

fn draw_sftp(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let layout = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(area);
    let connection = if app.connected_profile.is_some() {
        Span::styled("● 已连接", Style::default().fg(GREEN))
    } else if app.sftp_busy {
        Span::styled("◌ 正在处理", Style::default().fg(YELLOW))
    } else {
        Span::styled("○ 尚未连接", Style::default().fg(MUTED))
    };
    let host = app
        .connected_profile
        .as_ref()
        .map(|profile| format!("  {}", profile_target(profile)))
        .unwrap_or_else(|| "  从 SSH Manager 选择主机后按 s".to_owned());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            connection,
            Span::styled(host, Style::default().fg(SUBTEXT)),
        ]))
        .style(Style::default().bg(BG)),
        layout[0].inner(Margin {
            horizontal: 1,
            vertical: 0,
        }),
    );
    if let Some(progress) = app.transfer_progress.as_ref() {
        frame.render_widget(
            Paragraph::new(progress.as_str())
                .alignment(Alignment::Right)
                .style(Style::default().fg(YELLOW).bg(BG)),
            layout[0].inner(Margin {
                horizontal: 1,
                vertical: 0,
            }),
        );
    }

    let panels = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .spacing(1)
        .split(layout[1].inner(Margin {
            horizontal: 1,
            vertical: 0,
        }));
    draw_file_panel(
        frame,
        panels[0],
        "本地",
        &app.local_panel,
        app.sftp_side == SftpSide::Local,
        app.terminal_height,
    );
    draw_file_panel(
        frame,
        panels[1],
        "远端",
        &app.remote_panel,
        app.sftp_side == SftpSide::Remote,
        app.terminal_height,
    );

    let footer = "Tab 切栏  ↑↓/jk 选择  Enter 打开  Backspace 上级  u 上传  d 下载  m 新建目录  r 改名  x 删除  F5 刷新  c 取消  Esc 返回";
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(MUTED).bg(BG)),
        layout[2].inner(Margin {
            horizontal: 1,
            vertical: 0,
        }),
    );
}

fn draw_file_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    panel: &SftpPanelView,
    active: bool,
    terminal_height: u16,
) {
    let block = panel_block(
        format!("{} {title}", if active { "●" } else { "○" }),
        active,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(format!(" {}", panel.path))
            .style(Style::default().fg(LAVENDER).bg(ELEVATED)),
        layout[0],
    );
    let header = Table::new(
        [Row::new(["名称", "大小", "修改时间"]).style(Style::default().fg(MUTED))],
        [
            Constraint::Percentage(65),
            Constraint::Percentage(18),
            Constraint::Min(5),
        ],
    )
    .column_spacing(0);
    frame.render_widget(header, layout[1]);

    let count = usize::from(terminal_height.saturating_sub(10)).max(3);
    let start = panel
        .selected
        .saturating_sub(count / 2)
        .min(panel.entries.len().saturating_sub(count));
    let rows = panel
        .entries
        .iter()
        .enumerate()
        .skip(start)
        .take(count)
        .map(|(index, entry)| {
            let selected = index == panel.selected;
            let marker = match entry.kind {
                EntryKind::Directory => "▸",
                EntryKind::Link => "↗",
                EntryKind::File | EntryKind::Other => " ",
            };
            Row::new([
                Cell::from(format!(
                    "{} {marker} {}",
                    if selected { "›" } else { " " },
                    entry.name
                )),
                Cell::from(if entry.kind == EntryKind::Directory {
                    "<DIR>".to_owned()
                } else {
                    human_size(entry.size)
                })
                .style(Style::default().fg(SUBTEXT)),
                Cell::from(entry.modified.as_deref().unwrap_or_default())
                    .style(Style::default().fg(MUTED)),
            ])
            .style(
                Style::default()
                    .fg(if selected { BLUE } else { TEXT })
                    .bg(if selected { SELECTED } else { SURFACE }),
            )
        });
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(65),
            Constraint::Percentage(18),
            Constraint::Min(5),
        ],
    )
    .column_spacing(0)
    .style(Style::default().bg(SURFACE));
    frame.render_widget(table, layout[2]);
    if panel.loading {
        frame.render_widget(
            Paragraph::new(" 正在读取目录…").style(Style::default().fg(YELLOW).bg(SURFACE)),
            layout[2],
        );
    } else if !panel.error.is_empty() {
        frame.render_widget(
            Paragraph::new(format!(" {}", panel.error))
                .style(Style::default().fg(RED).bg(SURFACE))
                .wrap(Wrap { trim: true }),
            layout[2],
        );
    } else if panel.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(" （空目录）").style(Style::default().fg(MUTED).bg(SURFACE)),
            layout[2],
        );
    }
}

fn draw_modal(frame: &mut Frame<'_>, outer: Rect, modal: &Modal) {
    let height = match modal {
        Modal::Edit { .. } => 22,
        Modal::SftpCredentials { .. } => 11,
        _ => 7,
    }
    .min(outer.height.saturating_sub(2));
    let area = modal_rect(outer, height);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if modal.is_dangerous() { RED } else { BLUE }))
        .title(Span::styled(
            modal.title(),
            Style::default().fg(if modal.is_dangerous() { RED } else { BLUE }),
        ))
        .padding(Padding::uniform(1))
        .style(Style::default().fg(TEXT).bg(OVERLAY));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match modal {
        Modal::Filter(input) => {
            draw_simple_input_modal(
                frame,
                inner,
                "名称、分组、主机、用户或跳板机",
                input,
                "输入关键字",
            );
        }
        Modal::Quick(input) => {
            draw_simple_input_modal(
                frame,
                inner,
                "输入 [user@]host[:port]",
                input,
                "ops@example.com:22",
            );
        }
        Modal::Delete { profile } => {
            draw_confirmation(
                frame,
                inner,
                format!("确定删除「{}」？", profile.name),
                "Enter / y 确定，n / Esc 取消",
            );
        }
        Modal::SftpDelete { entry, .. } => {
            draw_confirmation(
                frame,
                inner,
                format!("递归删除「{}」？", entry.name),
                "Enter / y 确定，n / Esc 取消",
            );
        }
        Modal::SftpOverwrite { destination, .. } => {
            let name = destination
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(destination.as_str());
            draw_confirmation(
                frame,
                inner,
                format!("目标「{name}」已存在，覆盖吗？"),
                "Enter / y 覆盖，n / Esc 取消",
            );
        }
        Modal::SftpInput { mkdir, input, .. } => {
            draw_simple_input_modal(
                frame,
                inner,
                if *mkdir {
                    "输入新目录名称"
                } else {
                    "输入新名称"
                },
                input,
                "",
            );
        }
        Modal::SftpCredentials {
            password,
            jump_password,
            field,
            ..
        } => {
            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .split(inner);
            frame.render_widget(
                Paragraph::new("snapshot 不携带已保存密码；密钥和 Agent 会自动使用。")
                    .style(Style::default().fg(SUBTEXT).bg(OVERLAY)),
                rows[0],
            );
            draw_labeled_input(frame, rows[1], "目标密码", password, *field == 0);
            draw_labeled_input(frame, rows[2], "跳板机密码", jump_password, *field == 1);
            frame.render_widget(
                Paragraph::new("Tab 切字段 · 在第二项按 Enter 连接 · Esc 取消")
                    .style(Style::default().fg(MUTED).bg(OVERLAY)),
                rows[3],
            );
        }
        Modal::Edit {
            draft,
            field,
            cursors,
        } => draw_edit_modal(frame, inner, draft, *field, cursors),
    }
}

fn draw_simple_input_modal(
    frame: &mut Frame<'_>,
    area: Rect,
    help: &str,
    input: &TextInput,
    placeholder: &str,
) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
        .spacing(1)
        .split(area);
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(SUBTEXT).bg(OVERLAY)),
        rows[0],
    );
    draw_input(frame, rows[1], input, true, placeholder);
}

fn draw_confirmation(frame: &mut Frame<'_>, area: Rect, message: String, help: &str) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)])
        .spacing(1)
        .split(area);
    frame.render_widget(
        Paragraph::new(message).style(Style::default().fg(TEXT).bg(OVERLAY)),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(MUTED).bg(OVERLAY)),
        rows[1],
    );
}

fn draw_edit_modal(
    frame: &mut Frame<'_>,
    area: Rect,
    draft: &ProfileDraft,
    field: usize,
    cursors: &[usize; 8],
) {
    let labels = [
        "名称",
        "分组",
        "主机",
        "端口",
        "用户名",
        "认证",
        "密码",
        "跳板机",
    ];
    let values = [
        &draft.name,
        &draft.group,
        &draft.host,
        &draft.port,
        &draft.user,
        &draft.auth,
        &draft.password,
        &draft.jump_host,
    ];
    let placeholders = [
        "",
        "例如 prod/database",
        "",
        "",
        "",
        "agent / publicKey / password",
        "留空则不修改",
        "profile 或 user@host:port",
    ];
    let constraints = std::iter::repeat_n(Constraint::Length(2), labels.len())
        .chain([Constraint::Length(1)])
        .collect::<Vec<_>>();
    let rows = Layout::vertical(constraints).split(area);
    for index in 0..labels.len() {
        let input = TextInput {
            value: values[index].clone(),
            cursor: cursors[index].min(values[index].len()),
            secret: index == 6,
        };
        draw_labeled_input_with_placeholder(
            frame,
            rows[index],
            labels[index],
            &input,
            index == field,
            placeholders[index],
        );
    }
    frame.render_widget(
        Paragraph::new("Tab / Shift+Tab 切换字段 · Ctrl+S 保存 · Esc 取消")
            .style(Style::default().fg(MUTED).bg(OVERLAY)),
        rows[labels.len()],
    );
}

fn draw_labeled_input(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    input: &TextInput,
    focused: bool,
) {
    draw_labeled_input_with_placeholder(frame, area, label, input, focused, "不需要则留空");
}

fn draw_labeled_input_with_placeholder(
    frame: &mut Frame<'_>,
    area: Rect,
    label: &str,
    input: &TextInput,
    focused: bool,
    placeholder: &str,
) {
    let columns = Layout::horizontal([Constraint::Length(12), Constraint::Min(1)]).split(area);
    frame.render_widget(
        Paragraph::new(label).style(
            Style::default()
                .fg(if focused { BLUE } else { SUBTEXT })
                .bg(OVERLAY),
        ),
        columns[0],
    );
    draw_input(frame, columns[1], input, focused, placeholder);
}

fn draw_input(
    frame: &mut Frame<'_>,
    area: Rect,
    input: &TextInput,
    focused: bool,
    placeholder: &str,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let view = input_view(
        input,
        usize::from(area.width.saturating_sub(1)),
        placeholder,
    );
    let fg = if input.value.is_empty() { MUTED } else { TEXT };
    frame.render_widget(
        Paragraph::new(format!(" {}", view.text)).style(Style::default().fg(fg).bg(if focused {
            ELEVATED
        } else {
            SURFACE
        })),
        area,
    );
    if focused {
        let x = area
            .x
            .saturating_add(1)
            .saturating_add(view.cursor_column as u16)
            .min(area.right().saturating_sub(1));
        frame.set_cursor_position(Position::new(x, area.y));
    }
}

#[derive(Debug, PartialEq, Eq)]
struct InputView {
    text: String,
    cursor_column: usize,
}

fn input_view(input: &TextInput, width: usize, placeholder: &str) -> InputView {
    if input.value.is_empty() {
        return InputView {
            text: placeholder.to_owned(),
            cursor_column: 0,
        };
    }

    let mut cursor = input.cursor.min(input.value.len());
    while !input.value.is_char_boundary(cursor) {
        cursor = cursor.saturating_sub(1);
    }
    let cursor_index = input.value[..cursor].graphemes(true).count();
    let graphemes = input
        .value
        .graphemes(true)
        .map(|grapheme| {
            if input.secret {
                ("•".to_owned(), 1)
            } else {
                (grapheme.to_owned(), UnicodeWidthStr::width(grapheme))
            }
        })
        .collect::<Vec<_>>();
    let prefix_width = graphemes
        .iter()
        .take(cursor_index)
        .map(|(_, width)| *width)
        .sum::<usize>();
    let max_cursor_column = width.saturating_sub(1);
    let mut start = 0;
    let mut removed_width = 0;
    while start < cursor_index && prefix_width.saturating_sub(removed_width) > max_cursor_column {
        removed_width += graphemes[start].1;
        start += 1;
    }

    let mut shown = String::new();
    let mut shown_width = 0usize;
    for (grapheme, grapheme_width) in graphemes.iter().skip(start) {
        if shown_width.saturating_add(*grapheme_width) > width {
            break;
        }
        shown.push_str(grapheme);
        shown_width += *grapheme_width;
    }
    InputView {
        text: shown,
        cursor_column: prefix_width
            .saturating_sub(removed_width)
            .min(max_cursor_column),
    }
}

fn panel_block<'a>(title: impl Into<Line<'a>>, active: bool) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if active { BLUE } else { BORDER }))
        .title_style(Style::default().fg(if active { BLUE } else { SUBTEXT }))
        .title(title)
        .style(Style::default().fg(TEXT).bg(SURFACE))
}

fn human_size(size: Option<u64>) -> String {
    let Some(size) = size else {
        return String::new();
    };
    if size < 1024 {
        format!("{size} B")
    } else if size < 1024 * 1024 {
        format!("{:.1} K", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1} M", size as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.1} G", size as f64 / 1024.0 / 1024.0 / 1024.0)
    }
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(height),
        Constraint::Min(0),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}

fn modal_rect(outer: Rect, height: u16) -> Rect {
    centered_rect(64, height, outer)
}

pub(crate) fn manager_regions(width: u16, height: u16) -> ManagerRegions {
    let outer = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .split(Rect::new(0, 0, width, height));
    manager_regions_from_area(outer[1], width >= 102)
}

fn manager_regions_from_area(area: Rect, show_details: bool) -> ManagerRegions {
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let columns = if show_details {
        Layout::horizontal([
            Constraint::Length(24),
            Constraint::Min(20),
            Constraint::Length(34),
        ])
        .spacing(1)
        .split(inner)
    } else {
        Layout::horizontal([Constraint::Length(24), Constraint::Min(20)])
            .spacing(1)
            .split(inner)
    };
    ManagerRegions {
        groups: columns[0],
        hosts: columns[1],
        details: show_details.then(|| columns[2]),
    }
}

pub(crate) fn manager_group_row_at(area: Rect, point: Position) -> Option<usize> {
    let rows = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    rows.contains(point)
        .then(|| usize::from(point.y.saturating_sub(rows.y)))
}

pub(crate) fn manager_host_row_at(area: Rect, point: Position) -> Option<usize> {
    let inner = area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let rows = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );
    rows.contains(point)
        .then(|| usize::from(point.y.saturating_sub(rows.y)))
}

pub(crate) fn edit_modal_field_at(width: u16, height: u16, point: Position) -> Option<usize> {
    let outer = Rect::new(0, 0, width, height);
    let modal = modal_rect(outer, 22.min(height.saturating_sub(2)));
    // The edit modal uses one-cell borders and one-cell padding.
    let inner = modal.inner(Margin {
        horizontal: 2,
        vertical: 2,
    });
    if point.x < inner.x.saturating_add(12) || point.x >= inner.right() {
        return None;
    }
    let relative_y = point.y.checked_sub(inner.y)?;
    let field = usize::from(relative_y / 2);
    (field < 8 && relative_y < inner.height).then_some(field)
}

pub(crate) fn sftp_regions(width: u16, height: u16) -> SftpRegions {
    let body = Rect::new(0, 3, width, height.saturating_sub(3));
    let vertical = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(body);
    let panels = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .spacing(1)
        .split(vertical[1].inner(Margin {
            horizontal: 1,
            vertical: 0,
        }));
    SftpRegions {
        local: panels[0],
        remote: panels[1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_view_scrolls_by_unicode_cells_and_keeps_cursor_visible() {
        let input = TextInput::new("甲乙丙丁");
        assert_eq!(
            input_view(&input, 5, ""),
            InputView {
                text: "丙丁".to_owned(),
                cursor_column: 4,
            }
        );

        let input = TextInput {
            value: "甲乙丙丁".to_owned(),
            cursor: "甲".len(),
            secret: false,
        };
        assert_eq!(
            input_view(&input, 5, ""),
            InputView {
                text: "甲乙".to_owned(),
                cursor_column: 2,
            }
        );
    }

    #[test]
    fn secret_input_view_counts_graphemes_not_bytes() {
        let input = TextInput::secret("密🙂码");
        assert_eq!(
            input_view(&input, 3, ""),
            InputView {
                text: "••".to_owned(),
                cursor_column: 2,
            }
        );
    }

    #[test]
    fn manager_regions_stay_inside_even_tiny_terminals() {
        for (width, height) in [(0, 0), (1, 1), (20, 4), (80, 10), (120, 30)] {
            let regions = manager_regions(width, height);
            for area in [regions.groups, regions.hosts]
                .into_iter()
                .chain(regions.details)
            {
                assert!(area.right() <= width);
                assert!(area.bottom() <= height);
            }
        }
    }

    #[test]
    fn manager_hit_testing_excludes_borders_and_host_header() {
        let regions = manager_regions(120, 20);
        assert_eq!(
            manager_group_row_at(
                regions.groups,
                Position::new(regions.groups.x + 1, regions.groups.y)
            ),
            None
        );
        assert_eq!(
            manager_group_row_at(
                regions.groups,
                Position::new(regions.groups.x + 1, regions.groups.y + 1)
            ),
            Some(0)
        );
        assert_eq!(
            manager_host_row_at(
                regions.hosts,
                Position::new(regions.hosts.x + 1, regions.hosts.y + 1)
            ),
            None
        );
        assert_eq!(
            manager_host_row_at(
                regions.hosts,
                Position::new(regions.hosts.x + 1, regions.hosts.y + 2)
            ),
            Some(0)
        );
    }

    #[test]
    fn edit_modal_hit_test_only_selects_input_rows() {
        let outer = Rect::new(0, 0, 120, 30);
        let modal = modal_rect(outer, 22);
        let inner = modal.inner(Margin {
            horizontal: 2,
            vertical: 2,
        });
        assert_eq!(
            edit_modal_field_at(120, 30, Position::new(inner.x + 12, inner.y + 6)),
            Some(3)
        );
        assert_eq!(
            edit_modal_field_at(120, 30, Position::new(inner.x + 11, inner.y + 6)),
            None
        );
        assert_eq!(edit_modal_field_at(20, 3, Position::new(0, 0)), None);
    }
}
