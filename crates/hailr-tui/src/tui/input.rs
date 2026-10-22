use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::app::{App, Target};

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Previous,
    Next,
    /// 0 から数えたボタンの位置
    Answer(usize),
    Delete,
    Attach,
    Open,
    Reply,
    Click(Target),
    Quit,
}

/// 返事を打っている間のキー
#[derive(Debug, PartialEq, Eq)]
pub enum Typing {
    Text(String),
    Erase,
    Send,
    Cancel,
    Quit,
}

/// 打っている途中の数字や `q` を回答や終了にしないよう、文字は全部枠に入れる。貼り付けは 1 つの塊で受ける
pub fn typing(event: &Event) -> Option<Typing> {
    let (code, modifiers) = match event {
        Event::Paste(text) => return Some(Typing::Text(text.clone())),
        Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            ..
        }) => (code, modifiers),
        _ => return None,
    };
    if modifiers.contains(KeyModifiers::CONTROL) {
        return (*code == KeyCode::Char('c')).then_some(Typing::Quit);
    }
    match code {
        KeyCode::Char(c) if !modifiers.contains(KeyModifiers::ALT) => {
            Some(Typing::Text(c.to_string()))
        }
        KeyCode::Tab => Some(Typing::Text(" ".into())),
        KeyCode::Backspace => Some(Typing::Erase),
        // Option+Enter などで書きかけを送らないよう、修飾のない Enter だけで送る
        KeyCode::Enter if modifiers.is_empty() => Some(Typing::Send),
        KeyCode::Esc => Some(Typing::Cancel),
        _ => None,
    }
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
    // Ctrl-D や Ctrl-\ も Char('d') や Char('4') として届くため、修飾のある押下で答えたり消したりしない
    let plain = modifiers.difference(KeyModifiers::SHIFT).is_empty();
    match code {
        KeyCode::Up | KeyCode::Char('k') => Some(Action::Previous),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::Next),
        KeyCode::Char(c @ '1'..='9') if plain => Some(Action::Answer(*c as usize - '1' as usize)),
        KeyCode::Char('d') if plain => Some(Action::Delete),
        KeyCode::Enter if plain => Some(Action::Attach),
        KeyCode::Char('o') if plain => Some(Action::Open),
        KeyCode::Char('r') if plain => Some(Action::Reply),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Quit),
        _ => None,
    }
}

/// 左ボタンの押下だけを見る。場所は直前の描画で引く
pub fn click(event: &Event, app: &App) -> Option<Action> {
    let Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        ..
    }) = event
    else {
        return None;
    };
    app.target_at(*column, *row).map(Action::Click)
}

pub fn is_press(event: &Event) -> bool {
    matches!(
        event,
        Event::Key(KeyEvent {
            kind: KeyEventKind::Press,
            ..
        })
    )
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
        assert_eq!(press(KeyCode::Char('1'), none), Some(Action::Answer(0)));
        assert_eq!(press(KeyCode::Char('9'), none), Some(Action::Answer(8)));
        assert_eq!(press(KeyCode::Char('0'), none), None);
        assert_eq!(press(KeyCode::Char('d'), none), Some(Action::Delete));
        assert_eq!(press(KeyCode::Char('d'), KeyModifiers::CONTROL), None);
        assert_eq!(press(KeyCode::Char('4'), KeyModifiers::CONTROL), None);
        assert_eq!(press(KeyCode::Char('1'), KeyModifiers::ALT), None);
        assert_eq!(press(KeyCode::Enter, none), Some(Action::Attach));
        assert_eq!(press(KeyCode::Enter, KeyModifiers::ALT), None);
        assert_eq!(press(KeyCode::Enter, KeyModifiers::CONTROL), None);
        assert_eq!(press(KeyCode::Char('o'), none), Some(Action::Open));
        assert_eq!(press(KeyCode::Char('o'), KeyModifiers::CONTROL), None);
        assert_eq!(press(KeyCode::Char('o'), KeyModifiers::ALT), None);
        assert_eq!(press(KeyCode::Char('q'), none), Some(Action::Quit));
        assert_eq!(
            press(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
        assert_eq!(press(KeyCode::Char('c'), none), None);
        let release = key(KeyCode::Char('q'), none, KeyEventKind::Release);
        assert_eq!(action(&release), None);
    }

    #[test]
    fn clicks_with_the_left_button_only() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        use ratatui::layout::Rect;

        let app = App::default();
        app.add_target(Rect::new(3, 1, 2, 1), Target::Button(0));
        let mouse = |kind, column, row| {
            Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        let left = MouseEventKind::Down(MouseButton::Left);
        assert_eq!(
            click(&mouse(left, 4, 1), &app),
            Some(Action::Click(Target::Button(0)))
        );
        assert_eq!(click(&mouse(left, 1, 4), &app), None, "column and row");
        for kind in [
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Down(MouseButton::Right),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Moved,
            MouseEventKind::ScrollDown,
        ] {
            assert_eq!(click(&mouse(kind, 4, 1), &app), None, "{kind:?}");
        }
    }

    #[test]
    fn types_every_character_while_replying() {
        let press = |code, modifiers| typing(&key(code, modifiers, KeyEventKind::Press));
        let none = KeyModifiers::NONE;
        for c in ['1', 'q', 'd', 'j', 'r', 'あ'] {
            assert_eq!(
                press(KeyCode::Char(c), none),
                Some(Typing::Text(c.to_string()))
            );
        }
        let shift = KeyModifiers::SHIFT;
        assert_eq!(
            press(KeyCode::Char('A'), shift),
            Some(Typing::Text("A".into()))
        );
        assert_eq!(press(KeyCode::Enter, none), Some(Typing::Send));
        for modifiers in [KeyModifiers::ALT, KeyModifiers::SHIFT] {
            assert_eq!(press(KeyCode::Enter, modifiers), None);
        }
        assert_eq!(press(KeyCode::Tab, none), Some(Typing::Text(" ".into())));
        assert_eq!(press(KeyCode::Esc, none), Some(Typing::Cancel));
        assert_eq!(press(KeyCode::Backspace, none), Some(Typing::Erase));
        let control = KeyModifiers::CONTROL;
        assert_eq!(press(KeyCode::Char('c'), control), Some(Typing::Quit));
        assert_eq!(press(KeyCode::Char('d'), control), None);
        assert_eq!(press(KeyCode::Char('x'), KeyModifiers::ALT), None);
        assert_eq!(press(KeyCode::Up, none), None);
        let released = typing(&key(KeyCode::Char('1'), none, KeyEventKind::Release));
        assert_eq!(released, None);
        let pasted = typing(&Event::Paste("a\nb".into()));
        assert_eq!(pasted, Some(Typing::Text("a\nb".into())));
        assert_eq!(
            action(&key(KeyCode::Char('r'), none, KeyEventKind::Press)),
            Some(Action::Reply)
        );
    }
}
