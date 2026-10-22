use hailr_core::protocol::{Question, QuestionBody};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListState, Paragraph, Wrap};

use super::app::App;
use crate::config::Side;
use crate::text::printable;

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const WARNING: Color = Color::Yellow;
const KEYS: &str = " ↑↓ select  q quit";
const KEYS_WHEN_EMPTY: &str = " q quit";

/// 見た目の決めごとは DESIGN.md
pub fn draw(frame: &mut Frame, app: &App, side: Side) {
    let [body, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let list_width = (u32::from(body.width) * 3 / 10).clamp(24, 40) as u16;
    let list_width = list_width.min(body.width);
    let (main, list) = match side {
        Side::Right => {
            let [main, list] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Length(list_width)])
                    .areas(body);
            (main, list)
        }
        Side::Left => {
            let [list, main] =
                Layout::horizontal([Constraint::Length(list_width), Constraint::Fill(1)])
                    .areas(body);
            (main, list)
        }
    };
    draw_list(frame, app, list);
    match app.selected() {
        Some(question) => draw_question(frame, app, question, main),
        None => draw_empty(frame, Block::bordered().border_style(MUTED), main),
    }
    let keys = if app.questions().is_empty() {
        KEYS_WHEN_EMPTY
    } else {
        KEYS
    };
    frame.render_widget(Line::styled(keys, MUTED), footer);
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title("Questions").border_style(MUTED);
    if app.questions().is_empty() {
        draw_empty(frame, block, area);
        return;
    }
    let titles = app
        .questions()
        .iter()
        .map(|q| Line::raw(printable(&q.body.title())));
    let list = List::new(titles)
        .block(block)
        .highlight_symbol("▶ ")
        .highlight_spacing(ratatui::widgets::HighlightSpacing::Always)
        .highlight_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD));
    let mut state = ListState::default().with_selected(app.selected_index());
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_question(frame: &mut Frame, app: &App, question: &Question, area: Rect) {
    let paragraph = Paragraph::new(body_text(&question.body)).wrap(Wrap { trim: false });
    let wrapped = paragraph.line_count(area.width.saturating_sub(2));
    let wrapped = u16::try_from(wrapped).unwrap_or(u16::MAX);
    let height = wrapped.saturating_add(2).min(area.height / 3).max(3);
    let [preview, asked] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(height)]).areas(area);

    let pane = Block::bordered()
        .title(printable(&question.pane_id.0))
        .border_style(MUTED);
    let inner = pane.inner(preview);
    let content = match app.shot() {
        Some(Ok(screen)) => {
            // 下端に合わせ、いちばん新しい行が見えるようにする
            let hidden = screen.lines.len().saturating_sub(inner.height as usize);
            let hidden = u16::try_from(hidden).unwrap_or(u16::MAX);
            Paragraph::new(screen.clone()).scroll((hidden, 0))
        }
        Some(Err(reason)) => Paragraph::new(Line::styled(
            format!("cannot read the pane: {}", printable(reason)),
            WARNING,
        )),
        None => Paragraph::new(""),
    };
    frame.render_widget(content.block(pane), preview);

    let block = Block::bordered()
        .title(printable(&question.body.title()))
        .border_style(ACCENT);
    frame.render_widget(paragraph.block(block), asked);
}

fn body_text(body: &QuestionBody) -> String {
    match body {
        QuestionBody::Ask { question, .. } => printable_lines(question),
        QuestionBody::Permission {
            description,
            input_preview,
            ..
        } => format!(
            "{}\n\n{}",
            printable_lines(description),
            printable_lines(input_preview)
        ),
    }
}

fn draw_empty(frame: &mut Frame, block: Block, area: Rect) {
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let line = Line::styled("No questions", MUTED).alignment(Alignment::Center);
    frame.render_widget(line, middle);
}

/// 行の区切りは残し、行の中の制御文字だけを空白にする
fn printable_lines(text: &str) -> String {
    text.lines().map(printable).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use hailr_core::protocol::{ClientId, PaneId, QuestionId};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::text::Text;
    use uuid::Uuid;

    use super::*;

    fn ask(pane: &str, title: &str, question: &str) -> Question {
        Question {
            id: QuestionId(Uuid::new_v4()),
            client_id: ClientId(Uuid::new_v4()),
            pane_id: PaneId(pane.into()),
            asked_at: 1,
            body: QuestionBody::Ask {
                title: title.into(),
                question: question.into(),
                options: vec![],
                links: vec![],
            },
        }
    }

    fn render(app: &App, side: Side, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app, side)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn says_there_are_no_questions_on_both_sides() {
        let rows = render(&App::default(), Side::Right, 80, 12);
        let row = rows.iter().find(|r| r.contains("No questions")).unwrap();
        assert_eq!(row.matches("No questions").count(), 2);
        assert!(rows[0].find("Questions").unwrap() > 40);
        assert_eq!(rows[11].trim_end(), " q quit");
    }

    #[test]
    fn puts_the_list_on_the_configured_side() {
        let mut app = App::default();
        app.set_questions(vec![ask("w1:p1", "Merge PR?", "Ship it?")]);
        let right = render(&app, Side::Right, 80, 12);
        let left = render(&app, Side::Left, 80, 12);
        assert_eq!(column(&right[0], "┌Questions"), 80 - 24);
        assert_eq!(column(&left[0], "┌Questions"), 0);
        assert_eq!(column(&left[0], "┌w1:p1"), 24);
        assert!(right.iter().any(|r| r.contains("▶ Merge PR?")));
        assert_eq!(
            column(&render(&app, Side::Right, 200, 12)[0], "┌Questions"),
            160
        );
        assert_eq!(
            column(&render(&app, Side::Right, 100, 12)[0], "┌Questions"),
            70
        );
    }

    /// 罫線は 1 文字 3 バイトなので、バイトではなく桁で数える
    fn column(row: &str, needle: &str) -> usize {
        let byte = row.find(needle).unwrap();
        row[..byte].chars().count()
    }

    #[test]
    fn caps_the_question_at_a_third_and_keeps_at_least_one_line() {
        let mut app = App::default();
        app.set_questions(vec![ask("w1:p1", "Long", &"word ".repeat(500))]);
        let rows = render(&app, Side::Right, 80, 31);
        let top = rows.iter().position(|r| r.contains("┌Long")).unwrap();
        assert_eq!(top, 30 - 10, "a third of the 30 body rows");

        let rows = render(&app, Side::Right, 80, 7);
        let top = rows.iter().position(|r| r.contains("┌Long")).unwrap();
        assert_eq!(top, 6 - 3, "border, one line, border");
    }

    #[test]
    fn shows_the_question_below_and_the_newest_preview_lines() {
        let mut app = App::default();
        let question = ask(
            "w1:p1",
            "Merge\u{1b}]0;x\u{7}PR?",
            "first line\nsecond line",
        );
        app.set_questions(vec![question]);
        let screen: Vec<String> = (1..=30).map(|n| format!("line {n}")).collect();
        app.set_shot((PaneId("w1:p1".into()), Ok(Text::raw(screen.join("\n")))));

        let rows = render(&app, Side::Right, 80, 20);
        let text = rows.join("\n");
        assert!(text.contains("line 30"), "{text}");
        assert!(!text.contains("line 1 "), "{text}");
        let asked = rows
            .iter()
            .position(|r| r.contains("┌Merge ]0;x PR?"))
            .unwrap();
        assert!(rows[asked + 1].contains("first line"));
        assert!(rows[asked + 2].contains("second line"));
        assert!(rows.iter().position(|r| r.contains("line 30")).unwrap() < asked);
    }

    #[test]
    fn explains_a_preview_that_cannot_be_read() {
        let mut app = App::default();
        app.set_questions(vec![ask("w1:p1", "t", "q")]);
        app.set_shot((PaneId("w1:p1".into()), Err("herdr is gone".into())));
        let rows = render(&app, Side::Right, 80, 12);
        assert!(
            rows.iter()
                .any(|r| r.contains("cannot read the pane: herdr is gone"))
        );
    }

    #[test]
    fn fits_word_wrapped_questions_and_keeps_permission_lines() {
        let mut app = App::default();
        let mut permission = ask("w1:p1", "t", "q");
        permission.body = QuestionBody::Permission {
            request_id: "abcde".into(),
            tool_name: "Bash".into(),
            description: "Run the whole integration suite against staging databases".into(),
            input_preview: "pnpm test\n--filter api".into(),
        };
        app.set_questions(vec![permission]);
        let rows = render(&app, Side::Right, 60, 40);
        let text = rows.join("\n");
        assert!(text.contains("databases"), "{text}");
        assert!(rows.iter().any(|r| r.contains("│pnpm test")), "{text}");
        assert!(rows.iter().any(|r| r.contains("│--filter api")), "{text}");
    }

    #[test]
    fn draws_tiny_screens_without_panicking() {
        let mut app = App::default();
        app.set_questions(vec![ask("w1:p1", "t", &"long ".repeat(2000))]);
        for (width, height) in [(1, 1), (10, 3), (30, 5), (500, 2)] {
            render(&app, Side::Left, width, height);
            render(&App::default(), Side::Right, width, height);
        }
    }
}
