/// 制御文字と、向きを変える文字・行の区切り・見えない書式文字。Claude や pane が書いた文字列を端末に出す前に空白にする
pub fn is_unprintable(c: char) -> bool {
    c.is_control() || is_hidden(c)
}

/// 描くと端末を操作したり表示を偽ったりできる文字を空白にする
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if is_unprintable(c) { ' ' } else { c })
        .collect()
}

/// 向きを変える文字、行の区切り、見えない書式文字
fn is_hidden(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blanks_hidden_characters_only() {
        for c in [
            '\u{061C}',
            '\u{200B}',
            '\u{200F}',
            '\u{2028}',
            '\u{202E}',
            '\u{2060}',
            '\u{2069}',
            '\u{FEFF}',
            '\u{E0000}',
            '\u{E007F}',
        ] {
            assert!(is_hidden(c), "{:X}", c as u32);
        }
        for c in [
            '\u{200A}', '\u{2027}', '\u{202F}', '\u{205F}', '\u{206A}', 'あ',
        ] {
            assert!(!is_hidden(c), "{:X}", c as u32);
        }
    }

    #[test]
    fn printable_blanks_controls() {
        assert_eq!(
            printable("a\u{1b}]52;c;x\u{7}\u{202e}b\u{9b}2J\u{7f}c"),
            "a ]52;c;x  b 2J c"
        );
    }
}
