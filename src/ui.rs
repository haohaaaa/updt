use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use std::io;
use std::time::Duration;
use tokio::task::block_in_place;

use crate::output::MsgKind;
use crate::parse::strip_ansi_control_sequences;
use crate::state::{
    AppState, TargetStateFlags, profile_name, target_label, target_state_flags,
    target_update_summary, updatable_items_for_target,
};

pub type AppTerminal = Terminal<CrosstermBackend<io::Stdout>>;

pub struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    pub async fn enter() -> io::Result<Self> {
        block_in_place(|| {
            enable_raw_mode()?;
            execute!(io::stdout(), EnterAlternateScreen)?;
            Ok(Self { active: true })
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}

pub fn interrupted_error() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "user requested exit")
}

pub fn is_ctrl_exit_key(key: &KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return false;
    }
    matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
}

const TUI_KEY_TIMEOUT: Duration = Duration::from_millis(250);

async fn draw_terminal<F>(terminal: &mut AppTerminal, mut render: F) -> io::Result<()>
where
    F: FnMut(&mut ratatui::Frame<'_>),
{
    block_in_place(|| terminal.draw(|frame| render(frame)).map(|_| ()))
}

async fn read_key_event(timeout: Duration) -> io::Result<Option<KeyEvent>> {
    block_in_place(|| {
        if !event::poll(timeout)? {
            return Ok(None);
        }
        let Event::Key(key) = event::read()? else {
            return Ok(None);
        };
        Ok(Some(key))
    })
}

async fn read_pressed_key_event(timeout: Duration) -> io::Result<Option<KeyEvent>> {
    let Some(key) = read_key_event(timeout).await? else {
        return Ok(None);
    };
    if key.kind != KeyEventKind::Press {
        return Ok(None);
    }
    if is_ctrl_exit_key(&key) {
        return Err(interrupted_error());
    }
    Ok(Some(key))
}

pub fn summarize_target_status(target: &str, state: &AppState) -> (MsgKind, &'static str) {
    target_state_flags(state, target)
        .map(|flags| summarize_known_target_status(target, flags))
        .unwrap_or((MsgKind::Warn, "未知状态"))
}

fn summarize_known_target_status(target: &str, flags: TargetStateFlags) -> (MsgKind, &'static str) {
    if target_skipped(flags) {
        (MsgKind::Warn, "已跳过")
    } else {
        summarize_checked_target_status(target, flags)
    }
}

fn target_skipped(flags: TargetStateFlags) -> bool {
    !flags.enabled || !flags.installed
}

fn summarize_checked_target_status(
    target: &str,
    flags: TargetStateFlags,
) -> (MsgKind, &'static str) {
    if flags.needs_cargo_updater {
        (MsgKind::Warn, "缺少 cargo-update")
    } else if flags.check_failed {
        (MsgKind::Warn, "检查失败")
    } else if flags.has_updates {
        (MsgKind::Warn, target_update_summary(target))
    } else {
        (MsgKind::Ok, "当前最新")
    }
}

fn target_row_index(upgradable_targets: &[String], target_idx: usize, state: &AppState) -> usize {
    upgradable_targets
        .iter()
        .take(target_idx)
        .map(|target| 1 + updatable_items_for_target(state, target).len())
        .sum()
}

pub async fn select_targets_tui(
    terminal: &mut AppTerminal,
    state: &AppState,
    upgradable_targets: &[String],
) -> io::Result<Vec<String>> {
    select_targets_tui_with_checks(terminal, state, upgradable_targets, &[], "").await
}

pub async fn select_targets_tui_with_checks(
    terminal: &mut AppTerminal,
    state: &AppState,
    upgradable_targets: &[String],
    check_targets: &[String],
    start_time: &str,
) -> io::Result<Vec<String>> {
    if upgradable_targets.is_empty() {
        return Ok(Vec::new());
    }

    run_selection_list_loop(
        terminal,
        state,
        upgradable_targets,
        check_targets,
        start_time,
    )
    .await
}

async fn run_selection_list_loop(
    terminal: &mut AppTerminal,
    state: &AppState,
    upgradable_targets: &[String],
    check_targets: &[String],
    start_time: &str,
) -> io::Result<Vec<String>> {
    let mut cursor = 0usize;
    let mut selected = vec![true; upgradable_targets.len()];

    loop {
        let view = SelectionListView {
            state,
            upgradable_targets,
            selected: &selected,
            cursor,
            check_targets,
            start_time,
        };
        draw_terminal(terminal, |frame| render_selection_view(frame, &view)).await?;

        let Some(key) = read_pressed_key_event(TUI_KEY_TIMEOUT).await? else {
            continue;
        };
        if let Some(chosen) =
            handle_selection_key(&key, upgradable_targets, &mut cursor, &mut selected)
        {
            return Ok(chosen);
        }
    }
}

struct SelectionListView<'a> {
    state: &'a AppState,
    upgradable_targets: &'a [String],
    selected: &'a [bool],
    cursor: usize,
    check_targets: &'a [String],
    start_time: &'a str,
}

fn render_selection_view(frame: &mut ratatui::Frame<'_>, view: &SelectionListView<'_>) {
    if selection_view_shows_checks(view) {
        render_selection_with_checks(frame, view);
    } else {
        render_selection_simple(frame, view);
    }
}

fn selection_view_shows_checks(view: &SelectionListView<'_>) -> bool {
    !view.check_targets.is_empty() && !view.start_time.is_empty()
}

fn render_selection_with_checks(frame: &mut ratatui::Frame<'_>, view: &SelectionListView<'_>) {
    let area = frame.area();
    let targets_height = checks_target_list_height(area, view.check_targets);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(targets_height),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(area);

    render_check_summary(
        frame,
        chunks[0],
        chunks[1],
        view.state,
        view.check_targets,
        view.start_time,
    );
    render_selection_help(frame, chunks[2]);
    render_upgradable_selection_list(frame, chunks[3], view, "    - ");
}

fn render_selection_simple(frame: &mut ratatui::Frame<'_>, view: &SelectionListView<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(frame.area());

    render_selection_help(frame, chunks[0]);
    render_upgradable_selection_list(frame, chunks[1], view, "  - ");
}

fn checks_target_list_height(area: Rect, check_targets: &[String]) -> u16 {
    ((check_targets.len() as u16) + 2).clamp(3, area.height.saturating_sub(7))
}

fn render_check_summary(
    frame: &mut ratatui::Frame<'_>,
    header_area: Rect,
    list_area: Rect,
    state: &AppState,
    targets: &[String],
    start_time: &str,
) {
    render_check_summary_header(frame, header_area, state, targets, start_time);
    render_target_status_list(frame, list_area, state, targets);
}

fn render_check_summary_header(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    targets: &[String],
    start_time: &str,
) {
    let header = Paragraph::new(format!(
        "开始时间: {start_time}\n系统策略: {}\n进度: {}/{}",
        profile_name(state.system_profile),
        targets.len(),
        targets.len()
    ))
    .block(Block::default().title("检查可升级项").borders(Borders::ALL));
    frame.render_widget(header, area);
}

fn render_target_status_list(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    state: &AppState,
    targets: &[String],
) {
    let list = List::new(target_status_items(state, targets))
        .block(Block::default().title("目标").borders(Borders::ALL));
    frame.render_widget(list, area);
}

fn target_status_items(state: &AppState, targets: &[String]) -> Vec<ListItem<'static>> {
    targets
        .iter()
        .map(|target| target_status_item(state, target))
        .collect()
}

fn target_status_item(state: &AppState, target: &str) -> ListItem<'static> {
    let (kind, summary) = summarize_target_status(target, state);
    ListItem::new(format!("{:<10} {}", target_label(target), summary))
        .style(target_status_style(kind))
}

fn target_status_style(kind: MsgKind) -> Style {
    match kind {
        MsgKind::Info => Style::default().fg(Color::Cyan),
        MsgKind::Ok => Style::default().fg(Color::Green),
        MsgKind::Warn => Style::default().fg(Color::Yellow),
    }
}

fn render_selection_help(frame: &mut ratatui::Frame<'_>, area: Rect) {
    let help = Paragraph::new("↑/↓ 移动 · Space 勾选 · Enter 查看确认 · q/Esc 退出")
        .block(Block::default().title("updt").borders(Borders::ALL));
    frame.render_widget(help, area);
}

fn render_upgradable_selection_list(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    view: &SelectionListView<'_>,
    item_indent: &str,
) {
    let list = List::new(selection_list_items(view, item_indent))
        .block(
            Block::default()
                .title(format!(
                    "选择升级目标 · 已选 {}/{}",
                    view.selected.iter().filter(|v| **v).count(),
                    view.selected.len()
                ))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    let mut list_state = ListState::default();
    list_state.select(Some(target_row_index(
        view.upgradable_targets,
        view.cursor,
        view.state,
    )));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn selection_list_items(view: &SelectionListView<'_>, item_indent: &str) -> Vec<ListItem<'static>> {
    view.upgradable_targets
        .iter()
        .enumerate()
        .flat_map(|(idx, target)| selection_target_rows(view, target, idx, item_indent))
        .collect()
}

fn selection_target_rows(
    view: &SelectionListView<'_>,
    target: &str,
    idx: usize,
    item_indent: &str,
) -> Vec<ListItem<'static>> {
    let mut rows = vec![
        ListItem::new(format!(
            "{} {}",
            selection_mark(view.selected[idx]),
            target_label(target)
        ))
        .style(Style::default().add_modifier(Modifier::BOLD)),
    ];
    rows.extend(
        updatable_items_for_target(view.state, target)
            .into_iter()
            .map(|pkg| {
                ListItem::new(format!("{item_indent}{pkg}")).style(Style::default().fg(Color::Gray))
            }),
    );
    rows
}

fn selection_mark(selected: bool) -> &'static str {
    if selected { "[x]" } else { "[ ]" }
}

fn handle_selection_key(
    key: &KeyEvent,
    upgradable_targets: &[String],
    cursor: &mut usize,
    selected: &mut [bool],
) -> Option<Vec<String>> {
    if key.code == KeyCode::Up {
        *cursor = cursor.saturating_sub(1);
        return None;
    }
    if key.code == KeyCode::Down {
        move_selection_down(cursor, upgradable_targets);
        return None;
    }
    handle_selection_command_key(key, upgradable_targets, *cursor, selected)
}

fn move_selection_down(cursor: &mut usize, upgradable_targets: &[String]) {
    if *cursor + 1 < upgradable_targets.len() {
        *cursor += 1;
    }
}

fn handle_selection_command_key(
    key: &KeyEvent,
    upgradable_targets: &[String],
    cursor: usize,
    selected: &mut [bool],
) -> Option<Vec<String>> {
    if key.code == KeyCode::Char(' ') {
        selected[cursor] = !selected[cursor];
        return None;
    }
    if key.code == KeyCode::Enter {
        return Some(selected_upgradable_targets(upgradable_targets, selected));
    }
    selection_cancel_key(&key.code).then(Vec::new)
}

fn selected_upgradable_targets(upgradable_targets: &[String], selected: &[bool]) -> Vec<String> {
    upgradable_targets
        .iter()
        .zip(selected)
        .filter(|(_, selected)| **selected)
        .map(|(target, _)| target.clone())
        .collect()
}

fn selection_cancel_key(code: &KeyCode) -> bool {
    matches!(code, KeyCode::Esc | KeyCode::Char('q'))
}

pub async fn wait_tui_message(
    terminal: &mut AppTerminal,
    title: &str,
    lines: &[String],
) -> io::Result<bool> {
    let status = tui_status_text(title, lines);
    loop {
        draw_terminal(terminal, |frame| render_footer_status(frame, &status)).await?;
        if let Some(response) = read_message_response().await? {
            return Ok(response);
        }
    }
}

fn tui_status_text(title: &str, lines: &[String]) -> String {
    let body = clean_status_body(lines);
    format!("[{title}] {body}")
}

fn clean_status_body(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| strip_ansi_control_sequences(line))
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("  ")
}

fn clean_display_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| strip_ansi_control_sequences(line))
        .collect()
}

fn render_footer_status(frame: &mut ratatui::Frame<'_>, status: &str) {
    let footer = footer_area(frame.area());
    frame.render_widget(Clear, footer);
    render_status_line(frame, footer, status);
}

fn footer_area(area: Rect) -> Rect {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area)[1]
}

fn render_status_line(frame: &mut ratatui::Frame<'_>, area: Rect, status: &str) {
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Yellow)),
        area,
    );
}

async fn read_message_response() -> io::Result<Option<bool>> {
    let Some(key) = read_pressed_key_event(TUI_KEY_TIMEOUT).await? else {
        return Ok(None);
    };
    Ok(message_key_response(&key.code))
}

fn message_key_response(code: &KeyCode) -> Option<bool> {
    if message_accept_key(code) {
        Some(true)
    } else if message_cancel_key(code) {
        Some(false)
    } else {
        None
    }
}

fn message_accept_key(code: &KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y')
    )
}

fn message_cancel_key(code: &KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Esc
            | KeyCode::Char('q')
            | KeyCode::Char('Q')
            | KeyCode::Char('n')
            | KeyCode::Char('N')
    )
}

pub async fn wait_tui_message_on_checks(
    terminal: &mut AppTerminal,
    state: &AppState,
    targets: &[String],
    start_time: &str,
    title: &str,
    lines: &[String],
) -> io::Result<bool> {
    let status = tui_status_text(title, lines);

    loop {
        draw_terminal(terminal, |frame| {
            render_message_on_checks(frame, state, targets, start_time, &status)
        })
        .await?;
        if let Some(response) = read_message_response().await? {
            return Ok(response);
        }
    }
}

fn render_message_on_checks(
    frame: &mut ratatui::Frame<'_>,
    state: &AppState,
    targets: &[String],
    start_time: &str,
    status: &str,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_check_summary(frame, chunks[0], chunks[1], state, targets, start_time);
    frame.render_widget(Clear, chunks[2]);
    render_status_line(frame, chunks[2], status);
}

pub struct ChecksConfirmView<'a> {
    pub state: &'a AppState,
    pub targets: &'a [String],
    pub start_time: &'a str,
    pub title: &'a str,
    pub lines: &'a [String],
}

pub async fn wait_tui_float_on_checks(
    terminal: &mut AppTerminal,
    view: &ChecksConfirmView<'_>,
) -> io::Result<bool> {
    let clean_lines = clean_display_lines(view.lines);
    let mut confirm_selected = true;

    loop {
        draw_terminal(terminal, |frame| {
            render_checks_confirm_view(frame, view, &clean_lines, confirm_selected);
        })
        .await?;

        let Some(key) = read_pressed_key_event(TUI_KEY_TIMEOUT).await? else {
            continue;
        };
        if let Some(response) = handle_confirm_key(&key.code, &mut confirm_selected) {
            return Ok(response);
        }
    }
}

fn render_checks_confirm_view(
    frame: &mut ratatui::Frame<'_>,
    view: &ChecksConfirmView<'_>,
    clean_lines: &[String],
    confirm_selected: bool,
) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(1)])
        .split(area);

    render_check_summary(
        frame,
        chunks[0],
        chunks[1],
        view.state,
        view.targets,
        view.start_time,
    );
    render_confirm_popup(frame, area, view.title, clean_lines, confirm_selected);
}

pub struct SelectionConfirmView<'a> {
    pub state: &'a AppState,
    pub check_targets: &'a [String],
    pub start_time: &'a str,
    pub upgradable_targets: &'a [String],
    pub selected_targets: &'a [String],
    pub title: &'a str,
    pub lines: &'a [String],
}

pub async fn wait_tui_float_on_selection(
    terminal: &mut AppTerminal,
    view: &SelectionConfirmView<'_>,
) -> io::Result<bool> {
    let clean_lines = clean_display_lines(view.lines);
    let mut confirm_selected = true;

    loop {
        draw_terminal(terminal, |frame| {
            render_selection_confirm_view(frame, view, &clean_lines, confirm_selected);
        })
        .await?;

        let Some(key) = read_pressed_key_event(TUI_KEY_TIMEOUT).await? else {
            continue;
        };
        if let Some(response) = handle_confirm_key(&key.code, &mut confirm_selected) {
            return Ok(response);
        }
    }
}

fn render_selection_confirm_view(
    frame: &mut ratatui::Frame<'_>,
    view: &SelectionConfirmView<'_>,
    clean_lines: &[String],
    confirm_selected: bool,
) {
    let area = frame.area();
    let chunks = selection_confirm_chunks(area, view.check_targets);
    render_check_summary(
        frame,
        chunks[0],
        chunks[1],
        view.state,
        view.check_targets,
        view.start_time,
    );
    render_selection_help(frame, chunks[2]);
    render_confirm_selection_list(frame, chunks[3], view);
    render_confirm_popup(frame, area, view.title, clean_lines, confirm_selected);
}

fn selection_confirm_chunks(area: Rect, check_targets: &[String]) -> std::rc::Rc<[Rect]> {
    let targets_height = checks_target_list_height(area, check_targets);
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(targets_height),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(area)
}

fn render_confirm_selection_list(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    view: &SelectionConfirmView<'_>,
) {
    let list = List::new(confirm_selection_items(view)).block(
        Block::default()
            .title("选择要升级的项目")
            .borders(Borders::ALL),
    );
    frame.render_widget(list, area);
}

fn confirm_selection_items(view: &SelectionConfirmView<'_>) -> Vec<ListItem<'static>> {
    view.upgradable_targets
        .iter()
        .map(|target| confirm_selection_item(view, target))
        .collect()
}

fn confirm_selection_item(view: &SelectionConfirmView<'_>, target: &str) -> ListItem<'static> {
    let mark = selection_mark(confirm_target_selected(view, target));
    ListItem::new(format!("{mark} {}", target_label(target)))
}

fn confirm_target_selected(view: &SelectionConfirmView<'_>, target: &str) -> bool {
    view.selected_targets
        .iter()
        .any(|selected| selected == target)
}

fn render_confirm_popup(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    clean_lines: &[String],
    confirm_selected: bool,
) {
    let popup = centered_popup_area(area, clean_lines.len());
    frame.render_widget(Clear, popup);
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    render_confirm_popup_inner(frame, inner, clean_lines, confirm_selected);
}

fn centered_popup_area(area: Rect, line_count: usize) -> Rect {
    let popup_height = ((line_count as u16) + 2).clamp(3, area.height.saturating_sub(2));
    let popup_width = area.width.saturating_mul(80) / 100;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height.saturating_sub(popup_height) / 2),
            Constraint::Length(popup_height),
            Constraint::Min(0),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(area.width.saturating_sub(popup_width) / 2),
            Constraint::Length(popup_width),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}

fn render_confirm_popup_inner(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    clean_lines: &[String],
    confirm_selected: bool,
) {
    let inner_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    frame.render_widget(Paragraph::new(clean_lines.join("\n")), inner_chunks[0]);
    render_confirm_buttons(frame, inner_chunks[1], confirm_selected);
}

fn render_confirm_buttons(frame: &mut ratatui::Frame<'_>, area: Rect, confirm_selected: bool) {
    let (confirm_style, cancel_style) = confirm_button_styles(confirm_selected);
    let buttons = Line::from(vec![
        Span::raw("  "),
        Span::styled("[ 确认 ]", confirm_style),
        Span::raw("    "),
        Span::styled("[ 取消 ]", cancel_style),
    ]);
    frame.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), area);
}

fn confirm_button_styles(confirm_selected: bool) -> (Style, Style) {
    if confirm_selected {
        (active_confirm_style(Color::Green), inactive_confirm_style())
    } else {
        (
            inactive_confirm_style(),
            active_confirm_style(Color::Yellow),
        )
    }
}

fn active_confirm_style(color: Color) -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(color)
        .add_modifier(Modifier::BOLD)
}

fn inactive_confirm_style() -> Style {
    Style::default().fg(Color::Gray)
}

fn handle_confirm_key(code: &KeyCode, confirm_selected: &mut bool) -> Option<bool> {
    if *code == KeyCode::Left {
        *confirm_selected = true;
        return None;
    }
    if matches!(code, KeyCode::Right | KeyCode::Tab) {
        *confirm_selected = false;
        return None;
    }
    confirm_key_response(code, *confirm_selected)
}

fn confirm_key_response(code: &KeyCode, confirm_selected: bool) -> Option<bool> {
    if *code == KeyCode::Enter {
        Some(confirm_selected)
    } else if matches!(code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q')) {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clean_status_body, confirm_key_response, message_key_response, selected_upgradable_targets,
        summarize_target_status,
    };
    use crate::output::MsgKind;
    use crate::state::AppState;
    use crossterm::event::KeyCode;

    #[test]
    fn summarize_target_status_reports_skipped_unknown_and_updates() {
        let state = AppState::default();
        let (_, skipped_summary) = summarize_target_status("brew", &state);
        let (_, unknown_summary) = summarize_target_status("missing", &state);

        assert_eq!(skipped_summary, "已跳过");
        assert_eq!(unknown_summary, "未知状态");

        let mut updated = AppState::default();
        updated.enable.rustup = true;
        updated.rustup.installed = true;
        updated.rustup.has_updates = true;
        let (kind, summary) = summarize_target_status("rustup", &updated);

        assert!(matches!(kind, MsgKind::Warn));
        assert_eq!(summary, "发现可升级项");
    }

    #[test]
    fn summarize_target_status_prioritizes_cargo_updater_warning() {
        let mut state = AppState::default();
        state.enable.cargo = true;
        state.cargo.installed = true;
        state.cargo.check_failed = true;
        state.cargo.has_updates = true;

        let (_, summary) = summarize_target_status("cargo", &state);

        assert_eq!(summary, "缺少 cargo-update");
    }

    #[test]
    fn selected_upgradable_targets_filters_by_selected_flags() {
        let targets = vec![
            "brew".to_string(),
            "cargo".to_string(),
            "rustup".to_string(),
        ];
        let selected = vec![true, false, true];

        assert_eq!(
            selected_upgradable_targets(&targets, &selected),
            vec!["brew".to_string(), "rustup".to_string()]
        );
    }

    #[test]
    fn message_and_confirm_key_responses_follow_prompt_semantics() {
        assert_eq!(message_key_response(&KeyCode::Char('y')), Some(true));
        assert_eq!(message_key_response(&KeyCode::Char('N')), Some(false));
        assert_eq!(message_key_response(&KeyCode::Char('x')), None);

        assert_eq!(confirm_key_response(&KeyCode::Enter, true), Some(true));
        assert_eq!(confirm_key_response(&KeyCode::Enter, false), Some(false));
        assert_eq!(confirm_key_response(&KeyCode::Esc, true), Some(false));
        assert_eq!(confirm_key_response(&KeyCode::Tab, true), None);
    }

    #[test]
    fn clean_status_body_strips_empty_lines() {
        let lines = vec![" first ".to_string(), "".to_string(), "second".to_string()];

        assert_eq!(clean_status_body(&lines), " first   second");
    }
}
