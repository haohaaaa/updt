//! Mouse routing shares its hit regions with the renderer. UI chrome stays
//! local; only live terminal contents can send mouse reports to a child PTY.
use super::navigation::Panel;
use super::{
    View, content_regions, regions, session_list_offset, sidebar_regions, target_list_offset,
};
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{layout::Rect, text::Span, widgets::Block};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    Key(KeyCode),
    SelectSession(usize),
    SelectTarget(usize),
    Focus(Panel),
    ScrollOutput(bool),
    ScrollSessions(bool),
    ScrollTargets(bool),
    Forward(Vec<u8>),
}

pub(super) fn footer_buttons(
    footer: Rect,
    workflow: bool,
    confirm_exit: bool,
) -> Vec<(Rect, &'static str, KeyCode)> {
    let labels = if confirm_exit {
        vec![
            ("[Y 确认退出]", KeyCode::Char('y')),
            ("[N 返回]", KeyCode::Char('n')),
        ]
    } else if workflow {
        vec![
            ("[F2 记录]", KeyCode::F(2)),
            ("[F4 实时]", KeyCode::F(4)),
            ("[F5 上个]", KeyCode::F(5)),
            ("[F6 下个]", KeyCode::F(6)),
            ("[q 退出]", KeyCode::Char('q')),
        ]
    } else {
        vec![
            ("[F4 实时]", KeyCode::F(4)),
            ("[q 退出]", KeyCode::Char('q')),
        ]
    };
    if footer.height < 2 {
        return Vec::new();
    }
    let mut x = footer.x;
    labels
        .into_iter()
        .filter_map(|(label, key)| {
            let width = Span::raw(label).width() as u16;
            let area = Rect::new(x, footer.y + 1, width, 1);
            x = x.saturating_add(width + 1);
            // A clipped action is neither rendered nor clickable.
            (area.right() <= footer.right()).then_some((area, label, key))
        })
        .collect()
}

pub(super) fn action(
    mouse: MouseEvent,
    area: Rect,
    screen: &vt100::Screen,
    view: &View,
    workflow: bool,
    confirm_exit: bool,
    running: bool,
) -> Option<Action> {
    let position = (mouse.column, mouse.row).into();
    let (sidebar, pane, footer) = regions(area);
    let click = mouse.kind == MouseEventKind::Down(MouseButton::Left);
    if click {
        for (area, _, key) in footer_buttons(footer, workflow, confirm_exit) {
            if area.contains(position) {
                return Some(Action::Key(key));
            }
        }
    }
    if confirm_exit {
        return None;
    }
    let up = match mouse.kind {
        MouseEventKind::ScrollUp => Some(true),
        MouseEventKind::ScrollDown => Some(false),
        _ => None,
    };
    if sidebar.width > 0 {
        let (header, targets, list) = sidebar_regions(sidebar, workflow);
        if workflow && click && header.contains(position) {
            return Some(Action::Key(KeyCode::F(2)));
        }
        if targets.contains(position) {
            if let Some(up) = up {
                return Some(Action::ScrollTargets(up));
            }
            let inner = Block::bordered().inner(targets);
            if click && inner.contains(position) {
                let index = target_list_offset(view, targets) + usize::from(mouse.row - inner.y);
                return (index < view.target_count).then_some(Action::SelectTarget(index));
            }
            if click {
                return Some(Action::Focus(Panel::Targets));
            }
        }
        if workflow && list.contains(position) {
            if let Some(up) = up {
                return Some(Action::ScrollSessions(up));
            }
            let inner = Block::bordered().inner(list);
            if click && inner.contains(position) {
                let index = session_list_offset(view, list) + usize::from(mouse.row - inner.y);
                return (index < view.session_list.len()).then_some(Action::SelectSession(index));
            }
        }
    }
    let (_, pane) = content_regions(pane, workflow);
    if !pane.contains(position) {
        return None;
    }
    let inner = Block::bordered().inner(pane);
    let child_mouse = screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None;
    let shift = mouse.modifiers.contains(KeyModifiers::SHIFT);
    let live = running
        && ((view.focused && view.panel == Panel::Output) || click)
        && !view.history
        && screen.scrollback() == 0
        && view
            .session
            .as_ref()
            .is_none_or(|session| session.interactive);
    if let Some(up) = up
        && (!live || !child_mouse || shift)
    {
        return Some(Action::ScrollOutput(up));
    }
    if live
        && (!shift || up.is_none())
        && inner.contains(position)
        && let Some(bytes) = encode(mouse, inner, screen)
    {
        return Some(Action::Forward(bytes));
    }
    if click {
        return Some(Action::Focus(Panel::Output));
    }
    None
}

fn encode(mouse: MouseEvent, inner: Rect, screen: &vt100::Screen) -> Option<Vec<u8>> {
    use vt100::{MouseProtocolEncoding as Encoding, MouseProtocolMode as Mode};
    let mode = screen.mouse_protocol_mode();
    let button = |button| match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let release = matches!(mouse.kind, MouseEventKind::Up(_));
    let code = match mouse.kind {
        _ if mode == Mode::None => return None,
        MouseEventKind::Down(b) => button(b),
        _ if mode == Mode::Press => return None,
        MouseEventKind::Up(b) => button(b),
        MouseEventKind::Drag(b) if matches!(mode, Mode::ButtonMotion | Mode::AnyMotion) => {
            button(b) + 32
        }
        MouseEventKind::Moved if mode == Mode::AnyMotion => 35,
        MouseEventKind::ScrollUp => 64,
        MouseEventKind::ScrollDown => 65,
        MouseEventKind::ScrollLeft => 66,
        MouseEventKind::ScrollRight => 67,
        _ => return None,
    };
    // X10 reports only presses, without modifiers.
    let modifiers = if mode == Mode::Press {
        0
    } else {
        4 * u8::from(mouse.modifiers.contains(KeyModifiers::SHIFT))
            + 8 * u8::from(mouse.modifiers.contains(KeyModifiers::ALT))
            + 16 * u8::from(mouse.modifiers.contains(KeyModifiers::CONTROL))
    };
    let x = u32::from(mouse.column - inner.x) + 1;
    let y = u32::from(mouse.row - inner.y) + 1;
    if screen.mouse_protocol_encoding() == Encoding::Sgr {
        let suffix = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{};{x};{y}{suffix}", code + modifiers).into_bytes());
    }
    let code = if release { 3 } else { code } + modifiers;
    match screen.mouse_protocol_encoding() {
        Encoding::Default if x <= 223 && y <= 223 => {
            Some(vec![27, b'[', b'M', code + 32, x as u8 + 32, y as u8 + 32])
        }
        Encoding::Utf8 if x <= 2015 && y <= 2015 => {
            let mut bytes = vec![27, b'[', b'M', code + 32];
            for coordinate in [x, y] {
                let mut buffer = [0; 4];
                bytes.extend_from_slice(
                    char::from_u32(coordinate + 32)?
                        .encode_utf8(&mut buffer)
                        .as_bytes(),
                );
            }
            Some(bytes)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{SessionView, nonzero};
    use super::*;

    fn event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn parser(modes: &[u8]) -> vt100::Parser {
        let mut parser = vt100::Parser::new(nonzero(20), nonzero(68), 20);
        parser.process(modes);
        parser
    }

    #[test]
    fn mouse_reports_use_child_coordinates_modes_modifiers_and_release_encoding() {
        let inner = Rect::new(31, 5, 600, 30);
        let mut mouse = event(MouseEventKind::Down(MouseButton::Right), 35, 7);
        mouse.modifiers = KeyModifiers::ALT | KeyModifiers::CONTROL;
        let sgr = parser(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(
            encode(mouse, inner, sgr.screen()).unwrap(),
            b"\x1b[<26;5;3M"
        );
        mouse.kind = MouseEventKind::Up(MouseButton::Right);
        assert_eq!(
            encode(mouse, inner, sgr.screen()).unwrap(),
            b"\x1b[<26;5;3m"
        );
        mouse.kind = MouseEventKind::Drag(MouseButton::Right);
        assert_eq!(
            encode(mouse, inner, sgr.screen()).unwrap(),
            b"\x1b[<58;5;3M"
        );
        mouse.kind = MouseEventKind::Moved;
        assert!(encode(mouse, inner, sgr.screen()).is_none());
        let any = parser(b"\x1b[?1003h\x1b[?1006h");
        assert_eq!(
            encode(mouse, inner, any.screen()).unwrap(),
            b"\x1b[<59;5;3M"
        );
        let legacy = parser(b"\x1b[?1000h");
        mouse.kind = MouseEventKind::Up(MouseButton::Right);
        assert_eq!(
            encode(mouse, inner, legacy.screen()).unwrap(),
            [27, b'[', b'M', 59, 37, 35]
        );
        mouse.column = 300;
        assert!(encode(mouse, inner, legacy.screen()).is_none());
        let utf8 = parser(b"\x1b[?1000h\x1b[?1005h");
        assert_eq!(
            encode(mouse, inner, utf8.screen()).unwrap(),
            "\x1b[M;Į#".as_bytes()
        );
    }

    #[test]
    fn press_only_mode_ignores_drag_release_and_wheel() {
        let parser = parser(b"\x1b[?9h");
        let inner = Rect::new(1, 1, 20, 10);
        for kind in [
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::ScrollUp,
        ] {
            assert!(encode(event(kind, 2, 2), inner, parser.screen()).is_none());
        }
        let mut mouse = event(MouseEventKind::Down(MouseButton::Left), 2, 2);
        mouse.modifiers = KeyModifiers::CONTROL;
        assert_eq!(
            encode(mouse, inner, parser.screen()).unwrap(),
            b"\x1b[M \x22\x22"
        );
    }

    #[test]
    fn wheel_is_local_in_history_or_without_tracking_and_forwarded_only_when_focused() {
        let parser = parser(b"\x1b[?1000h\x1b[?1006h");
        let area = Rect::new(0, 0, 100, 24);
        let mut view = View::default();
        let mut mouse = event(MouseEventKind::ScrollUp, 35, 7);
        assert_eq!(
            action(mouse, area, parser.screen(), &view, true, false, true),
            Some(Action::ScrollOutput(true))
        );
        view.focused = true;
        assert_eq!(
            action(mouse, area, parser.screen(), &view, true, false, true),
            Some(Action::Forward(b"\x1b[<64;5;3M".to_vec()))
        );
        mouse.modifiers = KeyModifiers::SHIFT;
        assert_eq!(
            action(mouse, area, parser.screen(), &view, true, false, true),
            Some(Action::ScrollOutput(true))
        );
        mouse.modifiers = KeyModifiers::NONE;
        view.history = true;
        assert_eq!(
            action(mouse, area, parser.screen(), &view, true, false, true),
            Some(Action::ScrollOutput(true))
        );
        let click = event(MouseEventKind::Down(MouseButton::Left), 35, 7);
        assert_eq!(
            action(click, area, parser.screen(), &view, true, false, true),
            Some(Action::Focus(Panel::Output))
        );
        view.history = false;
        view.session = Some(SessionView {
            label: "capture".into(),
            code: None,
            interactive: false,
        });
        assert_eq!(
            action(click, area, parser.screen(), &view, true, false, true),
            Some(Action::Focus(Panel::Output))
        );
        assert_eq!(
            action(mouse, area, parser.screen(), &view, true, true, true),
            None
        );
        assert_eq!(
            action(
                event(MouseEventKind::ScrollUp, 100, 24),
                area,
                parser.screen(),
                &view,
                true,
                false,
                true
            ),
            None
        );
    }

    #[test]
    fn clicking_scrolled_sessions_and_targets_uses_the_visible_rows() {
        let parser = parser(b"");
        let area = Rect::new(0, 0, 100, 24);
        let mut view = View {
            session_offset: 7,
            session_list: (0..12).map(|i| format!("session {i}").into()).collect(),
            target_count: 12,
            selected_target: 10,
            ..View::default()
        };
        let (sidebar, _, _) = regions(area);
        let (_, targets, sessions) = sidebar_regions(sidebar, true);
        let inner = Block::bordered().inner(sessions);
        let click = event(
            MouseEventKind::Down(MouseButton::Left),
            inner.x,
            inner.y + 1,
        );
        assert_eq!(
            action(click, area, parser.screen(), &view, true, false, true),
            Some(Action::SelectSession(8))
        );
        view.session_offset = 99;
        assert_eq!(
            action(click, area, parser.screen(), &view, true, false, true),
            Some(Action::SelectSession(9))
        );
        let inner = Block::bordered().inner(targets);
        let click = event(
            MouseEventKind::Down(MouseButton::Left),
            inner.x,
            inner.bottom() - 1,
        );
        assert_eq!(
            action(click, area, parser.screen(), &view, true, false, true),
            Some(Action::SelectTarget(10))
        );
    }

    #[test]
    fn footer_hit_boxes_match_unicode_width_and_confirmation_blocks_background_actions() {
        let parser = parser(b"");
        for (width, height) in [(1, 1), (20, 4), (50, 12), (100, 24)] {
            let area = Rect::new(0, 0, width, height);
            let (_, _, footer) = regions(area);
            for confirm in [false, true] {
                for (button, _, key) in footer_buttons(footer, true, confirm) {
                    assert!(button.right() <= area.right());
                    assert_eq!(
                        action(
                            event(
                                MouseEventKind::Down(MouseButton::Left),
                                button.right() - 1,
                                button.y
                            ),
                            area,
                            parser.screen(),
                            &View::default(),
                            true,
                            confirm,
                            true
                        ),
                        Some(Action::Key(key))
                    );
                }
            }
        }
    }
}
