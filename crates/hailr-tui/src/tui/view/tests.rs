use hailr_core::protocol::PaneId;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::text::Text;

use super::*;
use crate::test_support::{self, QuestionExt};

fn ask(pane: &str, title: &str, question: &str) -> Question {
    test_support::ask().pane(pane).title(title).text(question)
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
    let permission = test_support::permission()
        .description("Run the whole integration suite against staging databases")
        .input_preview("pnpm test\n--filter api");
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
    let mut buttons = ask("w1:p1", "t", &"long ".repeat(2000));
    if let QuestionBody::Ask { options, .. } = &mut buttons.body {
        *options = (1..=9)
            .map(|n| format!("選択肢 {n} {}", "x".repeat(60)))
            .collect();
    }
    app.set_questions(vec![buttons]);
    for (width, height) in [(1, 1), (10, 3), (30, 5), (500, 2), (40, 12)] {
        render(&app, Side::Left, width, height);
        render(&App::default(), Side::Right, width, height);
    }
}

fn with_options(options: &[&str]) -> Question {
    ask("w1:p1", "Merge PR?", "Ship it?").options(options)
}

#[test]
fn marks_the_answered_question_and_takes_no_more_presses() {
    let mut app = App::default();
    let question = with_options(&["Yes, merge", "No"]);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    app.set_answered(question, Choice::Button(0), until);
    app.set_questions(vec![]);
    let rows = render(&app, Side::Right, 80, 16);
    assert!(
        rows.iter().any(|r| r.contains("┌Merge PR?  ✓ answered")),
        "{}",
        rows.join("\n")
    );
    let y = rows
        .iter()
        .position(|r| r.contains("[ 1 Yes, merge ] [ 2 No ]"))
        .unwrap();
    let x = rows[y].chars().position(|c| c == '[').unwrap();
    let (x, y) = (u16::try_from(x).unwrap(), u16::try_from(y).unwrap());
    assert_eq!(app.target_at(x, y), None);
    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
    terminal
        .draw(|frame| draw(frame, &app, Side::Right))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let pressed = &buffer[(x, y)];
    assert!(pressed.modifier.contains(Modifier::REVERSED));
    assert_eq!(pressed.fg, ACCENT);
    let other = rows[y as usize]
        .chars()
        .skip(usize::from(x) + 1)
        .position(|c| c == '[')
        .unwrap();
    let other_x = x + 1 + u16::try_from(other).unwrap();
    assert_eq!(app.target_at(other_x, y), None);
    let other = &buffer[(other_x, y)];
    assert!(!other.modifier.contains(Modifier::REVERSED));
    assert_eq!(other.fg, MUTED);
    assert_eq!(rows[15].trim_end(), " ↑↓ select  f focus  q quit");
}

#[test]
fn shows_the_reply_being_typed_with_the_cursor_at_its_end() {
    let mut app = App::default();
    app.set_questions(vec![with_options(&["Yes"])]);
    app.start_reply();
    app.type_reply("返事 ok");
    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
    terminal
        .draw(|frame| draw(frame, &app, Side::Right))
        .unwrap();
    let rows = render(&app, Side::Right, 80, 16);
    let y = rows.iter().position(|r| r.contains("┌Reply")).unwrap() + 1;
    // 幅 2 の文字は 2 つ目のセルが空になる
    assert!(rows[y].contains("返 事  ok"), "{}", rows.join("\n"));
    assert_eq!(rows[15].trim_end(), " Enter send  Esc cancel");
    let cursor = terminal.get_cursor_position().unwrap();
    let x = rows[y].chars().position(|c| c == '返').unwrap();
    assert_eq!(cursor.y, u16::try_from(y).unwrap());
    assert_eq!(cursor.x, u16::try_from(x).unwrap() + 7, "after 返事 ok");

    app.type_reply(&"x".repeat(200));
    let rows = render(&app, Side::Right, 80, 16);
    assert!(
        rows[y].contains("xxxx │"),
        "the end stays in view, with room for the cursor"
    );
}

#[test]
fn shows_a_sent_reply_greyed() {
    let mut app = App::default();
    let question = with_options(&["Yes"]);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    app.set_answered(question, Choice::Reply("later".into()), until);
    app.set_questions(vec![]);
    let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
    terminal
        .draw(|frame| draw(frame, &app, Side::Right))
        .unwrap();
    let rows = render(&app, Side::Right, 80, 16);
    let y = rows.iter().position(|r| r.contains("later")).unwrap();
    let x = rows[y].chars().position(|c| c == 'l').unwrap();
    let buffer = terminal.backend().buffer();
    let (x, y) = (u16::try_from(x).unwrap(), u16::try_from(y).unwrap());
    assert_eq!(buffer[(x, y)].fg, MUTED);
    let button = rows.iter().position(|r| r.contains("[ 1 Yes ]")).unwrap();
    let bx = rows[button].chars().position(|c| c == '[').unwrap();
    let (bx, by) = (u16::try_from(bx).unwrap(), u16::try_from(button).unwrap());
    assert_eq!(buffer[(bx, by)].fg, MUTED, "no button was pressed");
    assert_eq!(
        rows[15].trim_end(),
        " ↑↓ select  f focus  q quit",
        "the same keys after a reply"
    );
}

#[test]
fn tells_when_the_reply_has_nowhere_to_go() {
    let mut app = App::default();
    app.set_questions(vec![with_options(&["Yes"])]);
    app.start_reply();
    app.type_reply(&"h".repeat(crate::broker::MAX_REPLY + 1));
    assert!(app.notice().is_some());
    app.set_questions(vec![with_options(&["No"])]);
    let rows = render(&app, Side::Right, 80, 16);
    assert_eq!(rows[15].trim_end(), " the question went away  Esc close");
    let text = rows.join("\n");
    assert!(
        !text.contains("┌Reply"),
        "no box under another question\n{text}"
    );
    assert!(!text.contains("hhh"), "{text}");

    let mut app = App::default();
    let question = with_options(&["Yes"]);
    app.set_questions(vec![question.clone()]);
    app.start_reply();
    app.type_reply("half");
    app.set_questions(vec![]);
    app.set_questions(vec![question]);
    let rows = render(&app, Side::Right, 80, 16);
    let text = rows.join("\n");
    assert!(
        !text.contains("┌Reply"),
        "back on its own, not typeable\n{text}"
    );
    assert_eq!(rows[15].trim_end(), " the question went away  Esc close");
}

#[test]
fn shows_buttons_below_the_question_and_the_keys_to_press() {
    let mut app = App::default();
    app.set_questions(vec![with_options(&["Yes, merge", "No"])]);
    let rows = render(&app, Side::Right, 80, 16);
    let asked = rows.iter().position(|r| r.contains("┌Merge PR?")).unwrap();
    let buttons = rows
        .iter()
        .position(|r| r.contains("[ 1 Yes, merge ] [ 2 No ]"))
        .unwrap();
    assert!(buttons > asked, "{}", rows.join("\n"));
    assert_eq!(
        rows[15].trim_end(),
        " ↑↓ select  1-2 answer  r reply  f focus  d delete  q quit"
    );

    app.set_questions(vec![with_options(&["Yes"])]);
    assert!(app.just_moved());
    let rows = render(&app, Side::Right, 80, 16);
    assert_eq!(
        rows[15].trim_end(),
        " ↑↓ select  1 answer  r reply  f focus  d delete  q quit",
        "the keys of the question it moved to"
    );

    let mut app = App::default();
    app.set_questions(vec![with_options(&["Yes"])]);
    let rows = render(&app, Side::Right, 80, 16);
    assert_eq!(
        rows[15].trim_end(),
        " ↑↓ select  1 answer  r reply  f focus  d delete  q quit"
    );

    let mut app = App::default();
    app.set_questions(vec![with_options(&[])]);
    let rows = render(&app, Side::Right, 80, 16);
    let no_box = !rows.iter().any(|r| r.contains("[ 1"));
    assert!(no_box, "no box without options");
    assert_eq!(
        rows[15].trim_end(),
        " ↑↓ select  r reply  f focus  d delete  q quit"
    );
}

#[test]
fn says_when_buttons_do_not_fit() {
    let mut app = App::default();
    let long: Vec<String> = (1..=9).map(|n| format!("{n}{}", "x".repeat(40))).collect();
    let long: Vec<&str> = long.iter().map(String::as_str).collect();
    app.set_questions(vec![with_options(&long)]);
    let text = render(&app, Side::Right, 80, 16).join("\n");
    assert!(text.contains("some buttons are hidden"), "{text}");
    let rows = render(&app, Side::Right, 80, 16);
    let shown = app.shown_buttons();
    assert!(shown < 9, "{shown}");
    assert!(
        rows[15].contains(&format!("1-{shown} answer")),
        "{}",
        rows[15]
    );
    let text = render(&app, Side::Right, 80, 60).join("\n");
    assert!(!text.contains("some buttons are hidden"), "{text}");
    assert_eq!(app.shown_buttons(), 9);
    assert!(text.contains("[ 9 9"), "{text}");
}

#[test]
fn wraps_buttons_that_do_not_fit_and_shows_permission_buttons() {
    let mut app = App::default();
    let long: Vec<String> = (1..=6).map(|n| format!("option number {n}")).collect();
    let long: Vec<&str> = long.iter().map(String::as_str).collect();
    app.set_questions(vec![with_options(&long)]);
    let rows = render(&app, Side::Right, 80, 30);
    for n in 1..=6 {
        let button = format!("[ {n} option number {n} ]");
        assert!(
            rows.iter().any(|r| r.contains(&button)),
            "{}",
            rows.join("\n")
        );
    }

    let permission = test_support::permission();
    let mut app = App::default();
    app.set_questions(vec![permission]);
    let rows = render(&app, Side::Right, 80, 16);
    let text = rows.join("\n");
    assert!(text.contains("[ 1 Allow ] [ 2 Deny ]"), "{text}");
    assert_eq!(
        rows[15].trim_end(),
        " ↑↓ select  1-2 answer  f focus  d delete  q quit",
        "no reply to a permission request"
    );
}

#[test]
fn a_notice_takes_the_place_of_the_keys() {
    let mut app = App::default();
    app.set_questions(vec![with_options(&["Yes"])]);
    app.set_notice("the answer did not reach the session");
    let rows = render(&app, Side::Right, 80, 12);
    assert_eq!(rows[11].trim_end(), " the answer did not reach the session");
}

#[test]
fn counts_only_the_buttons_on_screen() {
    let mut app = App::default();
    let long: Vec<String> = (1..=9).map(|n| format!("{n}{}", "x".repeat(40))).collect();
    let long: Vec<&str> = long.iter().map(String::as_str).collect();
    app.set_questions(vec![with_options(&long)]);
    for (width, height) in [(80, 16), (80, 10), (80, 60), (27, 30)] {
        let text = render(&app, Side::Right, width, height).join("\n");
        let shown = app.shown_buttons();
        for n in 1..=shown {
            assert!(
                text.contains(&format!("[ {n} ")),
                "{width}x{height}: {text}"
            );
        }
        assert!(
            !text.contains(&format!("[ {} ", shown + 1)),
            "{width}x{height}: {text}"
        );
    }
    for width in [10, 24, 26] {
        render(&app, Side::Right, width, 30);
        assert_eq!(app.shown_buttons(), 0, "width {width}");
    }
}

fn with_links(options: &[&str], links: &[&str]) -> Question {
    let mut question = ask("w1:p1", "Merge PR?", "Ship it?");
    question.body = QuestionBody::Ask {
        title: "Merge PR?".into(),
        question: "Ship it?".into(),
        options: options.iter().map(|o| o.to_string()).collect(),
        links: links.iter().map(|l| l.to_string()).collect(),
    };
    question
}

#[test]
fn shortens_links() {
    for (link, short) in [
        (
            "https://github.com/mosmos21/hailr/pull/1",
            "mosmos21/hailr#1",
        ),
        (
            "https://github.com/mosmos21/hailr/issues/7/",
            "mosmos21/hailr#7",
        ),
        (
            "https://github.com/mosmos21/hailr/pull/1/files",
            "github.com/mosmos21/hailr/pull/1/files",
        ),
        (
            "https://github.com/mosmos21/hailr/pull/1#x",
            "github.com/mosmos21/hailr/pull/1#x",
        ),
        (
            "https://github.com/mosmos21/hailr/pull/new",
            "github.com/mosmos21/hailr/pull/new",
        ),
        (
            "http://github.com/mosmos21/hailr/pull/1",
            "http://github.com/mosmos21/hailr/pull/1",
        ),
        (
            "https://github.com/mosmos21/hailr",
            "github.com/mosmos21/hailr",
        ),
        ("https://github.com@evil.example/x", "evil.example/x"),
        ("https://u:p@example.com:8443/a", "example.com:8443/a"),
        (
            "https://example.com/docs/release/",
            "example.com/docs/release",
        ),
        ("http://example.com/a?b=c", "http://example.com/a?b=c"),
        ("https://app.example/#/admin", "app.example#/admin"),
        ("mailto:a", "mailto:a"),
        ("not a url\u{1b}", "not a url "),
    ] {
        assert_eq!(
            fit(&short_link(link), 100).as_deref(),
            Some(short),
            "{link}"
        );
    }
}

#[test]
fn lists_links_under_the_buttons() {
    let mut app = App::default();
    let links = [
        "https://github.com/mosmos21/hailr/pull/1",
        "https://example.com/docs",
    ];
    app.set_questions(vec![with_links(&["Yes", "No"], &links)]);
    let rows = render(&app, Side::Right, 100, 30);
    let text = rows.join("\n");
    let buttons = rows
        .iter()
        .position(|r| r.contains("[ 1 Yes ] [ 2 No ]"))
        .unwrap();
    assert!(rows[buttons + 1].contains("o1 mosmos21/hailr#1"), "{text}");
    assert!(rows[buttons + 2].contains("o2 example.com/docs"), "{text}");
    assert_eq!(app.shown_links(), 2);
    assert_eq!(
        rows[29].trim_end(),
        " ↑↓ select  1-2 answer  r reply  o open  f focus  d delete  q quit"
    );

    app.start_opening();
    let rows = render(&app, Side::Right, 100, 30);
    assert_eq!(rows[29].trim_end(), " 1-2 open link  any other key cancels");
}

#[test]
fn counts_only_the_links_on_screen() {
    let mut app = App::default();
    let links = [
        "https://a.example",
        "https://b.example",
        "https://c.example",
    ];
    app.set_questions(vec![with_links(&["Yes"], &links)]);
    // ボタンの枠は 1/3 の 4 行（中身 2 行）。ボタン 1 行と知らせ 1 行になる
    let rows = render(&app, Side::Right, 100, 13);
    let text = rows.join("\n");
    assert!(text.contains("some links are hidden"), "{text}");
    assert!(!text.contains("o1 "), "{text}");
    assert_eq!(app.shown_links(), 0);
    assert_eq!(app.shown_buttons(), 1);
    app.start_opening();
    let rows = render(&app, Side::Right, 100, 13);
    assert_eq!(
        rows[12].trim_end(),
        " no links on screen  any other key cancels"
    );

    let rows = render(&app, Side::Right, 100, 16);
    let text = rows.join("\n");
    assert!(text.contains("o1 a.example"), "{text}");
    assert!(!text.contains("o2 "), "{text}");
    assert_eq!(app.shown_links(), 1);
}

#[test]
fn keeps_the_host_when_a_link_does_not_fit() {
    let label = |link: &str| short_link(link);
    assert_eq!(
        label("http://a.example:8080/x?y#z"),
        Label {
            scheme: "http://",
            host: "a.example".into(),
            port: ":8080".into(),
            rest: "/x?y#z".into(),
            domain: true,
        }
    );
    let shown = |link: &str, room| fit(&label(link), room);
    assert_eq!(
        shown("https://a.example/docs", 14).as_deref(),
        Some("a.example/docs")
    );
    assert_eq!(
        shown("https://a.example/docs/release", 14).as_deref(),
        Some("a.example/doc…")
    );
    assert_eq!(
        shown("https://a.example/x", 10).as_deref(),
        Some("a.example…")
    );
    assert_eq!(
        shown("https://a.example/x", 9).as_deref(),
        Some("….example")
    );
    assert_eq!(
        shown("https://evilgithub.com/x", 11).as_deref(),
        Some("….com")
    );
    assert_eq!(
        shown("https://github.com.aaaa.evil.example/", 14).as_deref(),
        Some("….evil.example")
    );
    assert_eq!(
        shown("http://login.very-long.bank.example/", 20).as_deref(),
        Some("http://….example")
    );
    assert_eq!(shown("https://a.example/", 7), None);
    assert_eq!(shown("https://[::1]/x", 3), None);
    assert_eq!(shown("https://10.0.0.1/x", 5), None);
    assert_eq!(
        shown("https://evil.example./x", 10).as_deref(),
        Some("….example")
    );
    assert_eq!(
        shown("https://evil.example./x", 9).as_deref(),
        Some("….example")
    );
    assert_eq!(shown("https://evil.example./x", 8), None);
    assert_eq!(shown("https://evil.example.:8080/x", 10), None);
    assert_eq!(
        shown("https://a.example:8080/x", 14).as_deref(),
        Some("….example:8080")
    );
    assert_eq!(shown("https://a.example/x", 8), None);
    assert_eq!(shown("https://a.example/x", 0), None);
    let io = "https://github.com/mosmos21/user.github.io/pull/1";
    assert_eq!(shown(io, 23), None, "owner/repo is never cut");
    let pull = "https://github.com/mosmos21/hailr/pull/12";
    assert_eq!(shown(pull, 17).as_deref(), Some("mosmos21/hailr#12"));
    assert_eq!(shown(pull, 16).as_deref(), Some("mosmos21/hailr#…"));
    assert_eq!(shown(pull, 15).as_deref(), Some("mosmos21/hailr…"));
    assert_eq!(shown(pull, 14), None);
}

#[test]
fn stops_counting_links_at_one_whose_host_cannot_be_shown() {
    let counts = |links: &[&str]| {
        let question = with_links(&[], links);
        let rows = link_rows(&question.body, 12);
        rows.iter().map(|row| row.links).collect::<Vec<_>>()
    };
    let long = "https://a-very-long-host-name/x";
    assert_eq!(counts(&["https://a.b/", long]), [1, 0]);
    assert_eq!(counts(&[long, "https://a.b/"]), [0, 0]);
    assert_eq!(
        counts(&["https://255.255.255.255/", "https://a.b/"]),
        [0, 0]
    );
    assert_eq!(
        counts(&["https://github.com/mosmos21/hailr/pull/1", "https://a.b/"]),
        [0, 0]
    );

    let question = with_links(&[], &["https://a.b/", long]);
    let rows = link_rows(&question.body, 12);
    let numbers: Vec<_> = rows.iter().map(|row| row.line.spans[0].style.fg).collect();
    assert_eq!(numbers, [Some(ACCENT), Some(MUTED)]);
    assert_eq!(rows[1].line.to_string(), "o2 …");

    let question = with_links(&[], &[long, "https://a.b/"]);
    let rows = link_rows(&question.body, 12);
    let numbers: Vec<_> = rows.iter().map(|row| row.line.spans[0].style.fg).collect();
    assert_eq!(
        numbers,
        [Some(MUTED), Some(MUTED)],
        "after one that cannot open"
    );
    assert_eq!(rows[1].line.to_string(), "o2 a.b");
}

#[test]
fn records_where_to_click() {
    let mut app = App::default();
    let first = ask("w1:p1", "First", "q");
    let long = format!("https://{}/x", "h".repeat(60));
    let second = with_links(&["Yes", "No"], &["https://a.b/", &long]);
    app.set_questions(vec![first.clone(), second.clone()]);
    app.select(second.id);
    let rows = render(&app, Side::Right, 60, 30);
    let at = |text: &str| {
        let y = rows.iter().position(|r| r.contains(text)).unwrap();
        let x = rows[y].find(text).unwrap();
        let x = rows[y][..x].chars().count();
        (u16::try_from(x).unwrap(), u16::try_from(y).unwrap())
    };

    let (list_x, list_y) = at("First");
    assert_eq!(
        app.target_at(list_x, list_y),
        Some(Target::Question(first.id))
    );
    let (x, y) = at("[ 2 No ]");
    assert_eq!(app.target_at(x, y), Some(Target::Button(1)));
    assert_eq!(app.target_at(x + 7, y), Some(Target::Button(1)));
    assert_eq!(app.target_at(x - 1, y), None, "the gap between buttons");
    let (x, y) = at("o1 a.b");
    assert_eq!(app.target_at(x + 4, y), Some(Target::Link(0)));
    let (x, y) = at("o2 ");
    assert_eq!(app.target_at(x, y), None, "a link that cannot open");
    let (x, y) = at("Questions");
    assert_eq!(app.target_at(x, y), None, "the border");

    let (button_x, button_y) = at("[ 2 No ]");
    app.select(first.id);
    render(&app, Side::Right, 60, 30);
    assert_eq!(
        app.target_at(button_x, button_y),
        None,
        "a question without buttons"
    );
    app.set_questions(vec![]);
    render(&app, Side::Right, 60, 30);
    assert_eq!(app.target_at(list_x, list_y), None, "an empty list");
}

#[test]
fn clicks_the_rows_the_list_scrolled_to() {
    let mut app = App::default();
    let questions: Vec<_> = (0..20)
        .map(|i| ask("w1:p1", &format!("Question {i:02}"), "q"))
        .collect();
    app.set_questions(questions.clone());
    let rows = render(&app, Side::Right, 80, 12);
    assert_eq!(rows[1].chars().nth(56), Some('│'));
    let left = 56;
    let right = 79;
    assert_eq!(app.target_at(left, 1), None, "the left border");
    assert_eq!(app.target_at(right, 1), None, "the right border");
    assert_eq!(
        app.target_at(left + 1, 1),
        Some(Target::Question(questions[0].id))
    );
    assert_eq!(
        app.target_at(left + 1, 10),
        None,
        "the bottom border with more below"
    );

    app.select(questions[19].id);
    let rows = render(&app, Side::Right, 80, 12);
    let top = rows[1].clone();
    let title = &top[top.find("Question").unwrap()..][..11];
    assert_ne!(title, "Question 00", "the list scrolled");
    let x = u16::try_from(rows[1][..rows[1].find("Question").unwrap()].chars().count()).unwrap();
    let shown = questions.iter().find(|q| q.body.title() == title).unwrap();
    assert_eq!(app.target_at(x, 1), Some(Target::Question(shown.id)));
    assert_eq!(
        app.target_at(x, 9),
        Some(Target::Question(questions[19].id))
    );
    assert_eq!(app.target_at(x, 0), None, "the border");
    assert_eq!(app.target_at(x, 10), None, "the bottom border");
}
