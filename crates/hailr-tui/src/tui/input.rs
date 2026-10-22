use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Previous,
    Next,
    Quit,
}

/// raw mode では Ctrl-C がシグナルにならずキーとして届くため、ここで終わりにする
pub fn action(event: &Event) -> Option<Action> {
    let Event::Key(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        ..
    }) = event
    else {
        return None;
    };
    match code {
        KeyCode::Up | KeyCode::Char('k') => Some(Action::Previous),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::Next),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyEventState;

    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> Event {
        Event::Key(KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        })
    }

    #[test]
    fn maps_keys_on_press_only() {
        let press = |code, modifiers| action(&key(code, modifiers, KeyEventKind::Press));
        let none = KeyModifiers::NONE;
        assert_eq!(press(KeyCode::Up, none), Some(Action::Previous));
        assert_eq!(press(KeyCode::Char('k'), none), Some(Action::Previous));
        assert_eq!(press(KeyCode::Down, none), Some(Action::Next));
        assert_eq!(press(KeyCode::Char('j'), none), Some(Action::Next));
        assert_eq!(press(KeyCode::Char('q'), none), Some(Action::Quit));
        assert_eq!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
        assert_eq!(press(KeyCode::Char('c'), none), None);
        let release = key(KeyCode::Char('q'), none, KeyEventKind::Release);
        assert_eq!(action(&release), None);
    }
}
