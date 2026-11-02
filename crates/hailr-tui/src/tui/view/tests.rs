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
    assert!(rows[11].contains("q quit"), "{}", rows[11]);
    assert!(
        !rows[11].contains("delete") && !rows[11].contains("select"),
        "{}",
        rows[11]
    );
}

#[test]
fn puts_the_list_on_the_configured_side() {
    let mut app = App::default();
    app.set_questions(vec![ask("w1:p1", "Merge PR?", "Ship it?")]);
    let right = render(&app, Side::Right, 80, 12);
    let left = render(&app, Side::Left, 80, 12);
    let list_width = column(&left[0], "┌w1:p1");
    assert_eq!(column(&right[0], "┌Questions"), 80 - list_width);
    assert_eq!(column(&left[0], "┌Questions"), 0);
    assert!(right.iter().any(|r| r.contains("▶ Merge PR?")));

    let width_of = |total: u16| {
        let rows = render(&app, Side::Right, total, 12);
        total - u16::try_from(column(&rows[0], "┌Questions")).unwrap()
    };
    // DESIGN.md の仕様: 画面の 30%、24 桁より狭くせず 40 桁より広くしない
    assert_eq!(width_of(60), 24, "never narrower than 24 columns");
    assert_eq!(width_of(100), 30, "30% of the screen");
    assert_eq!(width_of(200), 40, "never wider than 40 columns");
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
    let box_height = |height: u16| {
        let rows = render(&app, Side::Right, 80, height);
        let top = rows.iter().position(|r| r.contains("┌Long")).unwrap();
        usize::from(height) - 1 - top
    };
    let (short, tall) = (box_height(31), box_height(61));
    // DESIGN.md の仕様: 一覧を除いた側の 1/3 まで
    assert_eq!(short, 10, "a third of 30 body rows");
    assert_eq!(tall, 20, "a third of 60 body rows");
    assert_eq!(box_height(7), 3, "border, one line, border");
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
    let footer = &rows[15];
    assert!(
        footer.contains("f focus") && footer.contains("q quit"),
        "{footer}"
    );
    for gone in ["answer", "d delete", "r reply"] {
        assert!(!footer.contains(gone), "{footer}");
    }
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
    assert!(rows[15].contains("Enter send"), "{}", rows[15]);
    assert!(rows[15].contains("Esc cancel"), "{}", rows[15]);
    assert!(!rows[15].contains("q quit"), "{}", rows[15]);
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
    assert!(
        rows[15].contains("f focus") && rows[15].contains("q quit"),
        "the same keys after a reply: {}",
        rows[15]
    );
    for gone in ["answer", "d delete", "r reply"] {
        assert!(!rows[15].contains(gone), "{}", rows[15]);
    }
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
    assert!(rows[15].contains("the question went away"), "{}", rows[15]);
    assert!(rows[15].contains("Esc close"), "{}", rows[15]);
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
    assert!(rows[15].contains("the question went away"), "{}", rows[15]);
    assert!(rows[15].contains("Esc close"), "{}", rows[15]);
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
    for key in ["1-2 answer", "r reply", "f focus", "d delete", "q quit"] {
        assert!(rows[15].contains(key), "{key}: {}", rows[15]);
    }

    app.set_questions(vec![with_options(&["Yes"])]);
    assert!(app.just_moved());
    let rows = render(&app, Side::Right, 80, 16);
    assert!(
        rows[15].contains("1 answer") && !rows[15].contains("1-2"),
        "the keys of the question it moved to: {}",
        rows[15]
    );

    let mut app = App::default();
    app.set_questions(vec![with_options(&["Yes"])]);
    let rows = render(&app, Side::Right, 80, 16);
    assert!(
        rows[15].contains("1 answer") && rows[15].contains("r reply"),
        "{}",
        rows[15]
    );

    let mut app = App::default();
    app.set_questions(vec![with_options(&[])]);
    let rows = render(&app, Side::Right, 80, 16);
    let no_box = !rows.iter().any(|r| r.contains("[ 1"));
    assert!(no_box, "no box without options");
    assert!(rows[15].contains("r reply"), "{}", rows[15]);
    assert!(!rows[15].contains("answer"), "{}", rows[15]);
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
    assert!(rows[15].contains("1-2 answer"), "{}", rows[15]);
    assert!(
        !rows[15].contains("reply"),
        "no reply to a permission request"
    );
}

#[test]
fn a_notice_takes_the_place_of_the_keys() {
    let mut app = App::default();
    app.set_questions(vec![with_options(&["Yes"])]);
    app.set_notice("the answer did not reach the session");
    let rows = render(&app, Side::Right, 80, 12);
    assert!(
        rows[11].contains("the answer did not reach the session"),
        "{}",
        rows[11]
    );
    assert!(!rows[11].contains("q quit"), "{}", rows[11]);
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
    assert!(rows[29].contains("o open"), "{}", rows[29]);
    assert!(rows[29].contains("1-2 answer"), "{}", rows[29]);

    app.start_opening();
    let rows = render(&app, Side::Right, 100, 30);
    assert!(rows[29].contains("1-2 open link"), "{}", rows[29]);
    assert!(rows[29].contains("any other key cancels"), "{}", rows[29]);
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
    assert!(rows[12].contains("no links on screen"), "{}", rows[12]);
    assert!(rows[12].contains("any other key cancels"), "{}", rows[12]);

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
    let left = column(&rows[0], "┌Questions");
    assert_eq!(rows[1].chars().nth(left), Some('│'));
    let left = u16::try_from(left).unwrap();
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

fn permission_rows(description: &str, input: &str, width: u16, height: u16) -> Vec<String> {
    let mut app = App::default();
    app.set_questions(vec![
        test_support::permission()
            .description(description)
            .input_preview(input),
    ]);
    render(&app, Side::Right, width, height)
}

#[test]
fn shows_a_json_input_by_key_before_the_first_sentence() {
    let input = r#"{"spreadsheetId":"abc","range":"A1:B2","values":[["x",1]],"n":3}"#;
    let rows = permission_rows("Sets values in a range. Needs write access.", input, 80, 30);
    let text = rows.join("\n");
    let line = |needle: &str| rows.iter().position(|r| r.contains(needle)).unwrap();
    // キーは入力の順のまま最長に揃え、文字列は引用符を外し、ほかは詰めた JSON
    assert!(
        rows.iter().any(|r| r.contains("│spreadsheetId  abc")),
        "{text}"
    );
    assert!(
        rows.iter().any(|r| r.contains("│range          A1:B2")),
        "{text}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains("│values         [[\"x\",1]]")),
        "{text}"
    );
    assert!(
        rows.iter().any(|r| r.contains("│n              3")),
        "{text}"
    );
    assert!(line("spreadsheetId") < line("range") && line("range") < line("values"));
    assert!(line("│n  ") < line("Sets values in a range."), "{text}");
    assert!(!text.contains("Needs write access"), "{text}");
    assert_eq!(rows[line("Sets values") - 1].trim_matches(['│', ' ']), "");
}

#[test]
fn greys_the_description_only() {
    let mut app = App::default();
    app.set_questions(vec![
        test_support::permission()
            .description("Sets values.")
            .input_preview(r#"{"a":"b"}"#),
    ]);
    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
    terminal
        .draw(|frame| draw(frame, &app, Side::Right))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let fg_of = |needle: &str| {
        let (y, row) = (0..20)
            .map(|y| {
                (
                    y,
                    (0..60).map(|x| buffer[(x, y)].symbol()).collect::<String>(),
                )
            })
            .find(|(_, row)| row.contains(needle))
            .unwrap();
        buffer[(u16::try_from(column(&row, needle)).unwrap(), y)].fg
    };
    assert_eq!(fg_of("Sets values."), MUTED);
    assert_ne!(fg_of("a  b"), MUTED);
}

#[test]
fn leaves_an_input_that_is_not_a_json_object_as_it_is() {
    for input in ["pnpm test\n--filter api", "[1,2]", "{}", "{\"a\":"] {
        let rows = permission_rows("Run it", input, 60, 20);
        let text = rows.join("\n");
        let first = input.lines().next().unwrap();
        let at = rows.iter().position(|r| r.contains(first)).unwrap();
        assert!(rows[at].contains(&format!("│{first}")), "{input:?}\n{text}");
        assert!(
            rows.iter().position(|r| r.contains("Run it")).unwrap() > at,
            "{text}"
        );
    }
}

#[test]
fn wraps_a_long_value_inside_the_value_column() {
    let input = r#"{"range":"A1","values":"0123456789abcdefghijklmnopqrstuvwxyz"}"#;
    // 内側の幅 28、キーの列 6 + 2 で値の列は 20
    let rows = permission_rows("d", input, 54, 20);
    let text = rows.join("\n");
    assert!(
        rows.iter()
            .any(|r| r.contains("│values  0123456789abcdefghij")),
        "{text}"
    );
    assert!(
        rows.iter().any(|r| r.contains("│        klmnopqrstuvwxyz")),
        "{text}"
    );
}

#[test]
fn keeps_the_input_visible_when_the_body_hits_its_cap() {
    let description = "word ".repeat(300);
    let rows = permission_rows(&description, r#"{"command":"ls"}"#, 80, 16);
    let text = rows.join("\n");
    assert!(rows.iter().any(|r| r.contains("│command  ls")), "{text}");
}

#[test]
fn blanks_control_characters_in_keys_and_values() {
    let input = r#"{"c":"a\u001b[2Jb\tc","k\u001b]0;x\u0007y":1}"#;
    let rows = permission_rows("d", input, 60, 40);
    let text = rows.join("\n");
    assert!(
        rows.iter().any(|r| r.contains("│c         a [2Jb c")),
        "{text}"
    );
    assert!(rows.iter().any(|r| r.contains("│k ]0;x y  1")), "{text}");
    assert!(
        !rows.concat().chars().any(crate::text::is_unprintable),
        "{text:?}"
    );
}

#[test]
fn aligns_and_wraps_double_width_characters() {
    let input = r#"{"キー":"あいうえおかきくけこ","ab":"x"}"#;
    // 内側の幅 18、キーの列 4 + 2 で値の列は 12（全角 6 文字）。全角は 2 セルで、2 つ目のセルは空白で描かれる
    let rows = permission_rows("d", input, 44, 40);
    let text = rows.join("\n");
    assert!(
        rows.iter()
            .any(|r| r.contains("│キ ー   あ い う え お か")),
        "{text}"
    );
    assert!(
        rows.iter().any(|r| r.contains("│      き く け こ")),
        "{text}"
    );
    assert!(rows.iter().any(|r| r.contains("│ab    x")), "{text}");
}

#[test]
fn keeps_a_grapheme_cluster_in_one_piece() {
    assert_eq!(wrap_graphemes("a❤\u{fe0f}b", 3), ["a❤\u{fe0f}", "b"]);
    assert_eq!(wrap_graphemes("", 3), [""]);
}

#[test]
fn takes_the_summary_up_to_a_newline_and_skips_leading_ones() {
    assert_eq!(summary("First line\nSecond. Third."), "First line");
    assert_eq!(summary("\n\nFirst. Second"), "First.");
    assert_eq!(summary("One. Two"), "One.");
    assert_eq!(summary("No stop"), "No stop");
    assert_eq!(summary(""), "");
}

#[test]
fn draws_a_json_permission_on_narrow_screens_without_panicking() {
    let input = r#"{"spreadsheetId":"abc","values":[["x",1]],"キー":"あいうえお"}"#;
    for (width, height) in [(1, 1), (3, 3), (10, 4), (20, 6), (30, 5), (40, 12)] {
        permission_rows("Sets values. More.", input, width, height);
    }
}

#[test]
fn shows_nested_values_and_numbers_as_they_were_sent() {
    let input = r#"{"o":{"z":1,"a":{"y":2,"b":3}},"n":1e2,"big":12345678901234567890123,"f":1.50,"t":true}"#;
    let rows = permission_rows("d", input, 80, 40);
    let text = rows.join("\n");
    for expected in [
        r#"│o    {"z":1,"a":{"y":2,"b":3}}"#,
        "│n    1e2",
        "│big  12345678901234567890123",
        "│f    1.50",
        "│t    true",
    ] {
        assert!(
            rows.iter().any(|r| r.contains(expected)),
            "{expected}\n{text}"
        );
    }
}

#[test]
fn shows_the_input_as_it_is_when_the_keys_leave_no_room() {
    let input = r#"{"averyveryverylongkeyname":"value"}"#;
    // 内側の幅 28 では、キーの列 26 + 2 に値の最低幅が残らない
    let rows = permission_rows("Run it", input, 54, 40);
    let text = rows.join("\n");
    assert!(
        rows.iter()
            .any(|r| r.contains("│{\"averyveryverylongkeyna")),
        "{text}"
    );
    assert!(!text.contains("│averyvery"), "{text}");
    let rows = permission_rows("Run it", input, 100, 40);
    assert!(
        rows.iter()
            .any(|r| r.contains("│averyveryverylongkeyname  value")),
        "{rows:?}"
    );
}

#[test]
fn splits_a_value_at_its_newlines() {
    let input = r#"{"body":"one\ntwo\n\nthree","k":"v"}"#;
    let rows = permission_rows("d", input, 60, 40);
    let text = rows.join("\n");
    let at = rows.iter().position(|r| r.contains("│body  one")).unwrap();
    assert!(rows[at + 1].contains("│      two"), "{text}");
    assert_eq!(rows[at + 2].trim_matches(['│', ' ']), "", "{text}");
    assert!(rows[at + 3].contains("│      three"), "{text}");
    assert!(rows[at + 4].contains("│k     v"), "{text}");
}

#[test]
fn shows_a_pretty_printed_nested_value_as_it_was_sent() {
    let input = "{\"o\": {\n  \"a\": 1\n}, \"k\": \"v\"}";
    let rows = permission_rows("d", input, 60, 40);
    let text = rows.join("\n");
    let at = rows.iter().position(|r| r.contains("│o  {")).unwrap();
    assert!(rows[at + 1].contains("│     \"a\": 1"), "{text}");
    assert!(rows[at + 2].contains("│   }"), "{text}");
    assert!(rows[at + 3].contains("│k  v"), "{text}");
}

#[test]
fn needs_exactly_the_minimum_value_width_for_a_row_per_key() {
    let input = r#"{"abcdefgh":"value"}"#;
    // 内側の幅 18 は、キーの列 8 + 2 に値の列が 8 桁残る最小の幅
    let rows = permission_rows("d", input, 44, 40);
    assert!(
        rows.iter().any(|r| r.contains("│abcdefgh  value")),
        "{rows:?}"
    );
    let rows = permission_rows("d", input, 43, 40);
    assert!(
        rows.iter().any(|r| r.contains("│{\"abcdefgh\":\"valu")),
        "{rows:?}"
    );
}

#[test]
fn aligns_keys_whose_graphemes_are_wider_than_the_string() {
    // unicode-width は `لا` を幅 1 と数えるが、ratatui は書記素ごとに 1 セルずつ置くため、この入力でキーの揃い方を確かめる
    assert!(Span::raw("لا").width() < cells("لا"));
    let rows = permission_rows("d", r#"{"لا":"x","abc":"y"}"#, 60, 40);
    assert!(rows.iter().any(|r| r.contains("│لا   x")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("│abc  y")), "{rows:?}");
}

#[test]
fn falls_back_to_the_quoted_text_when_a_string_cannot_be_decoded() {
    let rows = permission_rows("d", r#"{"s":"\ud800"}"#, 60, 40);
    assert!(
        rows.iter().any(|r| r.contains(r#"│s  "\ud800""#)),
        "{rows:?}"
    );
}

#[test]
fn ends_the_body_right_after_its_last_line() {
    let input = r#"{"body":"one\ntwo\n\nthree","k":"v"}"#;
    let mut app = App::default();
    app.set_questions(vec![
        test_support::permission()
            .description("d")
            .input_preview(input),
    ]);
    let rows = render(&app, Side::Right, 60, 40);
    let text = rows.join("\n");
    let at = rows.iter().position(|r| r.contains("│d")).unwrap();
    assert!(rows[at + 1].contains('└'), "{text}");
    let top = rows.iter().position(|r| r.contains("┌Allow")).unwrap();
    // 入力 5 行、空行、説明 1 行
    assert_eq!(at - top, 7, "{text}");
}

#[test]
fn adds_no_blank_line_when_the_description_or_the_input_is_empty() {
    let rows = permission_rows("", r#"{"k":"v"}"#, 60, 40);
    let at = rows.iter().position(|r| r.contains("│k  v")).unwrap();
    assert!(rows[at + 1].contains('└'), "{rows:?}");
    let rows = permission_rows("Run it", "", 60, 40);
    let at = rows.iter().position(|r| r.contains("│Run it")).unwrap();
    assert!(rows[at - 1].contains("┌Allow"), "{rows:?}");
    assert!(rows[at + 1].contains('└'), "{rows:?}");
}
