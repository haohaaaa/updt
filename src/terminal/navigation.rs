use super::{View, regions, sidebar_regions};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Panel {
    Targets,
    Sessions,
    Output,
}

pub(super) enum Action {
    Handled,
    SelectSession(usize),
    Exit,
}

pub(super) fn border(view: &View, panel: Panel) -> Style {
    Style::default().fg(if view.panel != panel {
        Color::DarkGray
    } else if view.focused {
        Color::Green
    } else {
        Color::Cyan
    })
}

pub(super) fn handle(key: KeyEvent, view: &mut View, area: Rect, workflow: bool) -> Option<Action> {
    if key.code == KeyCode::Esc {
        view.focused = false;
        return Some(Action::Handled);
    }
    let plain = !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    if !view.focused && plain && key.code == KeyCode::Char('q') {
        return Some(Action::Exit);
    }
    if !view.focused && key.code == KeyCode::Enter {
        view.focused = true;
        return Some(Action::Handled);
    }
    let direction = if plain {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => Some(PanelDirection::Left),
            KeyCode::Char('j') | KeyCode::Down => Some(PanelDirection::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(PanelDirection::Up),
            KeyCode::Char('l') | KeyCode::Right => Some(PanelDirection::Right),
            _ => None,
        }
    } else {
        None
    };
    if !view.focused {
        if let Some(direction) = direction {
            let (sidebar, _, _) = regions(area);
            if sidebar.width > 0 {
                let sessions = workflow && sidebar_regions(sidebar, workflow).2.height > 2;
                view.panel = match (view.panel, direction) {
                    (Panel::Output, PanelDirection::Left) => Panel::Targets,
                    (Panel::Targets, PanelDirection::Down) if sessions => Panel::Sessions,
                    (Panel::Sessions, PanelDirection::Up) => Panel::Targets,
                    (Panel::Targets | Panel::Sessions, PanelDirection::Right) => Panel::Output,
                    _ => view.panel,
                };
            }
        }
        return Some(Action::Handled);
    }
    if view.panel == Panel::Output {
        return None;
    }
    if let Some(direction) = direction {
        let previous = matches!(direction, PanelDirection::Up | PanelDirection::Left);
        let (index, count) = match view.panel {
            Panel::Targets => (view.selected_target, view.target_count),
            Panel::Sessions => (view.selected_session, view.session_list.len()),
            Panel::Output => unreachable!(),
        };
        let index = if previous {
            index.saturating_sub(1)
        } else {
            index.saturating_add(1)
        }
        .min(count.saturating_sub(1));
        if view.panel == Panel::Sessions && count > 0 {
            return Some(Action::SelectSession(index));
        }
        view.selected_target = index;
    }
    Some(Action::Handled)
}

enum PanelDirection {
    Left,
    Down,
    Up,
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn arrows_and_hjkl_navigate_panels_then_entries_after_enter() {
        for codes in [
            [
                KeyCode::Char('h'),
                KeyCode::Char('j'),
                KeyCode::Char('k'),
                KeyCode::Char('l'),
            ],
            [KeyCode::Left, KeyCode::Down, KeyCode::Up, KeyCode::Right],
        ] {
            let mut view = View {
                target_count: 4,
                session_list: vec!["workflow".into(), "command".into()],
                ..View::default()
            };
            let area = Rect::new(0, 0, 100, 24);
            handle(key(codes[0]), &mut view, area, true);
            assert_eq!(view.panel, Panel::Targets);
            handle(key(codes[1]), &mut view, area, true);
            assert_eq!(view.panel, Panel::Sessions);
            handle(key(KeyCode::Enter), &mut view, area, true);
            assert!(view.focused);
            assert!(matches!(
                handle(key(codes[1]), &mut view, area, true),
                Some(Action::SelectSession(1))
            ));
            assert_eq!(view.panel, Panel::Sessions);
            handle(key(KeyCode::Esc), &mut view, area, true);
            assert!(!view.focused);
            handle(key(codes[2]), &mut view, area, true);
            assert_eq!(view.panel, Panel::Targets);
            handle(key(KeyCode::Enter), &mut view, area, true);
            handle(key(codes[1]), &mut view, area, true);
            assert_eq!(view.selected_target, 1);
            handle(key(KeyCode::Esc), &mut view, area, true);
            handle(key(codes[3]), &mut view, area, true);
            assert_eq!(view.panel, Panel::Output);
        }
    }

    #[test]
    fn unfocused_input_is_consumed_and_focused_terminal_receives_navigation_and_q() {
        let area = Rect::new(0, 0, 100, 24);
        let mut view = View::default();
        assert!(matches!(
            handle(key(KeyCode::Char('x')), &mut view, area, true),
            Some(Action::Handled)
        ));
        assert!(matches!(
            handle(key(KeyCode::Char('q')), &mut view, area, true),
            Some(Action::Exit)
        ));
        handle(key(KeyCode::Enter), &mut view, area, true);
        for code in [
            KeyCode::Char('q'),
            KeyCode::Char('h'),
            KeyCode::Up,
            KeyCode::Enter,
        ] {
            assert!(handle(key(code), &mut view, area, true).is_none());
        }
        assert!(matches!(
            handle(key(KeyCode::Esc), &mut view, area, true),
            Some(Action::Handled)
        ));
        assert!(!view.focused);
        assert!(matches!(
            handle(key(KeyCode::Esc), &mut view, area, true),
            Some(Action::Handled)
        ));
    }

    #[test]
    fn navigation_skips_hidden_panels_and_clamps_entries() {
        let mut view = View {
            focused: true,
            panel: Panel::Targets,
            target_count: 2,
            ..View::default()
        };
        let area = Rect::new(0, 0, 100, 24);
        for _ in 0..5 {
            handle(key(KeyCode::Down), &mut view, area, true);
        }
        assert_eq!(view.selected_target, 1);
        for _ in 0..5 {
            handle(key(KeyCode::Up), &mut view, area, true);
        }
        assert_eq!(view.selected_target, 0);
        view = View::default();
        handle(key(KeyCode::Left), &mut view, Rect::new(0, 0, 20, 4), true);
        assert_eq!(view.panel, Panel::Output);
        handle(key(KeyCode::Left), &mut view, area, false);
        handle(key(KeyCode::Down), &mut view, area, false);
        assert_eq!(view.panel, Panel::Targets);
    }
}
