use hailr_core::protocol::{Question, QuestionBody};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListState, Paragraph, Wrap};

use super::app::{App, Choice, Target};
use crate::config::Side;
use crate::text::printable;

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const WARNING: Color = Color::Yellow;

/// 見た目の決めごとは DESIGN.md
pub fn draw(frame: &mut Frame, app: &App, side: Side) {
    app.clear_targets();
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
    let footer_line = match app.notice() {
        _ if app.reply_gone() => Line::styled(" the question went away  Esc close", WARNING),
        Some(notice) => Line::styled(format!(" {}", printable(notice)), WARNING),
        None => Line::styled(keys(app), MUTED),
    };
    frame.render_widget(footer_line, footer);
}

/// そのとき使えるキーだけを並べる
fn keys(app: &App) -> String {
    if app.reply().is_some() {
        return " Enter send  Esc cancel".into();
    }
    if app.selected().is_none() {
        return " q quit".into();
    }
    if app.opening() {
        return match app.shown_links() {
            0 => " no links on screen  any other key cancels".into(),
            1 => " 1 open link  any other key cancels".into(),
            n => format!(" 1-{n} open link  any other key cancels"),
        };
    }
    let answer = match app.shown_buttons() {
        0 => String::new(),
        1 => "  1 answer".into(),
        n => format!("  1-{n} answer"),
    };
    let open = if app.shown_links() > 0 {
        "  o open"
    } else {
        ""
    };
    if app.selected_is_answered() {
        return format!(" ↑↓ select{open}  f focus  q quit");
    }
    let reply = if app
        .selected()
        .is_some_and(|q| matches!(q.body, QuestionBody::Ask { .. }))
    {
        "  r reply"
    } else {
        ""
    };
    format!(" ↑↓ select{answer}{reply}{open}  f focus  d delete  q quit")
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title("Questions").border_style(MUTED);
    if app.questions().is_empty() {
        draw_empty(frame, block, area);
        return;
    }
    let inner = block.inner(area);
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
    let shown = app.questions().iter().skip(state.offset());
    for (y, question) in (inner.y..inner.bottom()).zip(shown) {
        let row = Rect::new(inner.x, y, inner.width, 1);
        app.add_target(row, Target::Question(question.id));
    }
}

fn draw_question(frame: &mut Frame, app: &App, question: &Question, area: Rect) {
    let paragraph = Paragraph::new(body_text(&question.body)).wrap(Wrap { trim: false });
    let wrapped = paragraph.line_count(area.width.saturating_sub(2));
    let wrapped = u16::try_from(wrapped).unwrap_or(u16::MAX);
    let height = wrapped.saturating_add(2).min(area.height / 3).max(3);
    let answered = app.answered_with(question.id);
    let typing = app
        .reply()
        .filter(|(id, _)| *id == question.id && !app.reply_gone())
        .map(|(_, text)| text);
    let sent = match answered {
        Some(Choice::Reply(text)) => Some(text.as_str()),
        _ => None,
    };
    let mut rows = button_rows(&question.body, area.width.saturating_sub(2), answered);
    rows.extend(link_rows(&question.body, area.width.saturating_sub(2)));
    let rows_height = match rows.len() {
        0 => 0,
        n => u16::try_from(n)
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .min(area.height / 3)
            .max(3),
    };
    let reply_height = if typing.is_some() || sent.is_some() {
        3
    } else {
        0
    };
    let [preview, asked, choices, reply] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Length(rows_height),
        Constraint::Length(reply_height),
    ])
    .areas(area);
    if let Some(text) = typing.or(sent) {
        draw_reply(frame, text, typing.is_some(), reply);
    }
    app.set_shown_buttons(0);
    app.set_shown_links(0);
    if !rows.is_empty() {
        let mut block = Block::bordered().border_style(MUTED);
        let inner = block.inner(choices);
        // `[ 1 ` が入らない幅では番号が見えないため、1 つも見えていないとみなす
        let visible = if inner.width < 4 {
            0
        } else {
            usize::from(inner.height)
        };
        if rows.len() > visible && visible == 1 {
            block = block.title(Line::styled("more hidden", WARNING));
        }
        // 隠れたボタンやリンクの番号では答えさせない・開かせないため、見えている数を App に渡し、隠れていることも出す
        if rows.len() > visible {
            let keep = if visible >= 2 { visible - 1 } else { visible };
            let hidden = rows.split_off(keep);
            if visible >= 2 {
                let what = if hidden.iter().any(|row| row.buttons > 0) {
                    "buttons"
                } else {
                    "links"
                };
                rows.push(Row {
                    line: Line::styled(
                        format!("some {what} are hidden; make the window taller"),
                        WARNING,
                    ),
                    buttons: 0,
                    links: 0,
                    targets: vec![],
                });
            }
        }
        app.set_shown_buttons(rows.iter().map(|row| row.buttons).sum());
        app.set_shown_links(rows.iter().map(|row| row.links).sum());
        if inner.width >= 4 {
            for (y, row) in (inner.y..inner.bottom()).zip(&rows) {
                for &(x, width, target) in &row.targets {
                    let area =
                        Rect::new(inner.x.saturating_add(x), y, width, 1).intersection(inner);
                    app.add_target(area, target);
                }
            }
        }
        let lines: Vec<Line> = rows.into_iter().map(|row| row.line).collect();
        frame.render_widget(Paragraph::new(lines).block(block), choices);
    }

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

    let mut title = Line::raw(printable(&question.body.title()));
    if answered.is_some() {
        title.push_span(Span::styled("  ✓ answered", ACCENT));
    }
    let block = Block::bordered().title(title).border_style(ACCENT);
    frame.render_widget(paragraph.block(block), asked);
}

/// ボタンの枠の 1 行と、その行にあるボタンとリンクの数
struct Row {
    line: Line<'static>,
    buttons: usize,
    links: usize,
    /// 行の中でクリックできる場所（左端からの位置と幅）
    targets: Vec<(u16, u16, Target)>,
}

/// 選択肢を `[ 1 Yes ]` の形で並べ、幅に入るだけ 1 行に詰める。ボタンの途中では折り返さない
fn button_rows(body: &QuestionBody, width: u16, answered: Option<&Choice>) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    for (i, option) in body.options().iter().enumerate() {
        let style = match answered {
            None => Style::new().fg(ACCENT),
            Some(&Choice::Button(pressed)) if pressed == i => {
                Style::new().fg(ACCENT).add_modifier(Modifier::REVERSED)
            }
            Some(_) => Style::new().fg(MUTED),
        };
        let button = Span::styled(format!("[ {} {} ]", i + 1, printable(option)), style);
        let size = u16::try_from(button.width()).unwrap_or(u16::MAX);
        let target = answered.is_none().then_some(Target::Button(i));
        match rows.last_mut() {
            Some(row) if row.line.width() + 1 + button.width() <= usize::from(width) => {
                let x = u16::try_from(row.line.width() + 1).unwrap_or(u16::MAX);
                row.line.push_span(" ");
                row.line.push_span(button);
                row.buttons += 1;
                row.targets.extend(target.map(|t| (x, size, t)));
            }
            _ => rows.push(Row {
                line: Line::from(button),
                buttons: 1,
                links: 0,
                targets: target.map(|t| (0, size, t)).into_iter().collect(),
            }),
        }
    }
    rows
}

/// 打っている間は末尾が見えるよう左を落とし、IME の候補が末尾に出るよう端末のカーソルを置く
fn draw_reply(frame: &mut Frame, text: &str, typing: bool, area: Rect) {
    let color = if typing { ACCENT } else { MUTED };
    let block = Block::bordered().title("Reply").border_style(color);
    let inner = block.inner(area);
    let room = usize::from(inner.width.saturating_sub(u16::from(typing)));
    let text = printable(text);
    let mut shown = String::new();
    let mut width = 0;
    for c in text.chars().rev() {
        let w = Span::raw(c.to_string()).width();
        if width + w > room {
            break;
        }
        width += w;
        shown.insert(0, c);
    }
    let style = if typing {
        Style::new()
    } else {
        Style::new().fg(MUTED)
    };
    frame.render_widget(
        Paragraph::new(Line::styled(shown, style)).block(block),
        area,
    );
    if typing && inner.width > 0 && inner.height > 0 {
        let x = inner
            .x
            .saturating_add(u16::try_from(width).unwrap_or(u16::MAX));
        frame.set_cursor_position((x.min(inner.right().saturating_sub(1)), inner.y));
    }
}

/// リンクは 1 行に 1 つ。ホストを出せないリンクとそれより後ろは開かせず、番号を控えめの色にする
fn link_rows(body: &QuestionBody, width: u16) -> Vec<Row> {
    let mut openable = true;
    body.links()
        .iter()
        .enumerate()
        .map(|(i, link)| {
            let number = format!("o{} ", i + 1);
            let room = usize::from(width).saturating_sub(number.len());
            let shown = fit(&short_link(link), room);
            openable &= shown.is_some();
            Row {
                line: Line::from(vec![
                    Span::styled(number, if openable { ACCENT } else { MUTED }),
                    Span::raw(shown.unwrap_or_else(|| "…".into())),
                ]),
                buttons: 0,
                links: usize::from(openable),
                targets: if openable {
                    vec![(0, width, Target::Link(i))]
                } else {
                    vec![]
                },
            }
        })
        .collect()
}

/// 縮めるときに残す scheme とホストと、縮めてよい残り
#[derive(Debug, PartialEq, Eq)]
struct Label {
    scheme: &'static str,
    host: String,
    port: String,
    rest: String,
    /// ドメイン名のホストだけは `.` の境目で左を落としてよい
    domain: bool,
}

/// 右端で切れて別のホストに見えないよう、ホストを残してパスを縮める。ホストも入らなければ `.` の境目で左を落とし、
/// それも入らなければ None
fn fit(label: &Label, room: usize) -> Option<String> {
    let Label {
        scheme,
        host,
        port,
        rest,
        domain,
    } = label;
    let head = format!("{scheme}{host}{port}");
    let (head_len, rest_len) = (head.chars().count(), rest.chars().count());
    if head_len + rest_len <= room {
        return Some(head + rest);
    }
    if head_len < room {
        let kept: String = rest.chars().take(room - head_len - 1).collect();
        return Some(format!("{head}{kept}…"));
    }
    if !domain {
        return None;
    }
    let left = room.checked_sub(scheme.len() + 1 + port.len())?;
    let host = host.trim_end_matches('.');
    let cut = host
        .char_indices()
        .filter(|&(_, c)| c == '.')
        .map(|(i, _)| &host[i..])
        .find(|suffix| suffix.chars().count() <= left)?;
    Some(format!("{scheme}…{cut}{port}"))
}

/// GitHub の PR / issue は `owner/repo#番号`、ほかはホストとパスで出す
fn short_link(link: &str) -> Label {
    let other = |text: &str| Label {
        scheme: "",
        host: String::new(),
        port: String::new(),
        rest: printable(text),
        domain: false,
    };
    let Ok(url) = url::Url::parse(link) else {
        return other(link);
    };
    let Some(host) = url.host_str() else {
        return other(link);
    };
    let path = url.path().trim_end_matches('/');
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    let plain = url.port().is_none() && url.query().is_none() && url.fragment().is_none();
    if url.scheme() == "https"
        && host == "github.com"
        && plain
        && let [owner, repo, "pull" | "issues", number] = segments.as_slice()
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
    {
        // どの repo かが読めないまま開かせないよう、owner/repo は縮めない
        return Label {
            scheme: "",
            host: printable(&format!("{owner}/{repo}")),
            port: String::new(),
            rest: printable(&format!("#{number}")),
            domain: false,
        };
    }
    // userinfo（`github.com@evil.example`）でホストを偽らせないよう出さない
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let query = url.query().map(|q| format!("?{q}")).unwrap_or_default();
    let fragment = url.fragment().map(|f| format!("#{f}")).unwrap_or_default();
    Label {
        scheme: if url.scheme() == "https" {
            ""
        } else {
            "http://"
        },
        host: printable(host),
        port,
        rest: printable(&format!("{path}{query}{fragment}")),
        domain: matches!(url.host(), Some(url::Host::Domain(_))),
    }
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
mod tests;
