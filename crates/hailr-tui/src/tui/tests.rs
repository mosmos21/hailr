use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use hailr_core::protocol::{ClientEvent, ClientId, Question, QuestionBody};
use uuid::Uuid;

use super::*;
use crate::broker::ClientGuard;
use crate::test_support::{self, QuestionExt};

fn ask(client: ClientId, options: &[&str]) -> Question {
    test_support::ask().client(client).options(options)
}

/// 1 つの client が接続した broker にする
fn connected() -> (
    Arc<Broker>,
    ClientId,
    mpsc::Receiver<ClientEvent>,
    ClientGuard,
) {
    let broker = Broker::new();
    let client = ClientId(Uuid::new_v4());
    let (events, guard) = broker.connect(client);
    (broker, client, events, guard)
}

/// broker の質問を読み込み、ボタンを全部出した状態にする
fn loaded(broker: &Broker) -> App {
    let mut app = App::default();
    app.set_questions(broker.snapshot());
    app.set_shown_buttons(9);
    app
}

/// 答えた質問を残す時間が過ぎたあとの一覧にする
fn wait_out(app: &mut App, broker: &Broker) {
    app.expire_answered(Instant::now() + ANSWERED_FOR, broker.snapshot());
}

/// 選んでいる質問にボタン 0 で答え（送られたことを確かめ）、答えた質問を残す時間を過ぎさせる
fn answer_and_wait_out(app: &mut App, broker: &Broker, events: &mut mpsc::Receiver<ClientEvent>) {
    apply(app, broker, Action::Answer(0));
    assert!(events.try_recv().is_ok());
    wait_out(app, broker);
}

/// 質問を並べて登録し、2 件目を選んだ状態にする
fn second_selected(broker: &Broker, questions: &[Question]) -> App {
    for q in questions {
        broker.register(q.clone()).unwrap();
    }
    let mut app = loaded(broker);
    apply(&mut app, broker, Action::Next);
    app
}

#[test]
fn keeps_an_answered_question_until_its_time_is_up() {
    let (broker, client, mut events, _guard) = connected();
    let first = ask(client, &["Yes", "No"]);
    let mut second = ask(client, &["Yes"]);
    second.asked_at = 2;
    let mut app = second_selected(&broker, &[first.clone(), second.clone()]);
    apply(&mut app, &broker, Action::Previous);

    apply(&mut app, &broker, Action::Answer(1));
    let answered = ClientEvent::Answered {
        id: first.id,
        answer: "No".into(),
    };
    assert_eq!(events.try_recv().ok(), Some(answered));
    assert_eq!(broker.snapshot(), vec![second.clone()]);
    assert_eq!(app.selected().map(|q| q.id), Some(first.id));
    assert_eq!(app.answered_with(first.id), Some(&Choice::Button(1)));
    assert!(!app.just_moved());

    for action in [
        Action::Delete,
        Action::Click(Target::Button(0)),
        Action::Answer(0),
    ] {
        apply(&mut app, &broker, action);
    }
    assert!(events.try_recv().is_err(), "pressed again");
    assert_eq!(
        app.notice(),
        None,
        "no warning for a press that does nothing"
    );
    assert_eq!(broker.snapshot(), vec![second.clone()]);
    app.expire_answered(Instant::now(), broker.snapshot());
    assert_eq!(
        app.selected().map(|q| q.id),
        Some(first.id),
        "gone too early"
    );

    wait_out(&mut app, &broker);
    assert_eq!(app.selected().map(|q| q.id), Some(second.id));
    assert!(app.just_moved());
}

#[test]
fn treats_a_question_asked_again_as_new() {
    let (broker, client, mut events, _guard) = connected();
    let question = ask(client, &["Yes"]);
    broker.register(question.clone()).unwrap();
    let mut app = loaded(&broker);

    apply(&mut app, &broker, Action::Answer(0));
    assert!(events.try_recv().is_ok());
    broker.register(question.clone()).unwrap();
    app.set_questions(broker.snapshot());
    assert_eq!(app.answered_with(question.id), None);
    assert!(app.just_moved(), "a held key must not answer it again");
    apply(&mut app, &broker, Action::Answer(0));
    assert!(events.try_recv().is_err());

    apply(&mut app, &broker, Action::Previous);
    apply(&mut app, &broker, Action::Answer(0));
    assert!(events.try_recv().is_ok());
}

#[test]
fn leaves_another_choice_alone_when_an_answered_question_comes_back() {
    let (broker, client, mut events, _guard) = connected();
    let first = ask(client, &["Yes"]);
    let mut second = ask(client, &["Yes"]);
    second.asked_at = 2;
    let mut app = second_selected(&broker, &[first.clone(), second.clone()]);
    apply(&mut app, &broker, Action::Previous);

    apply(&mut app, &broker, Action::Answer(0));
    assert!(events.try_recv().is_ok());
    apply(&mut app, &broker, Action::Next);
    broker.register(first.clone()).unwrap();
    app.set_questions(broker.snapshot());
    assert!(!app.just_moved());
    assert_eq!(app.selected().map(|q| q.id), Some(second.id));
    apply(&mut app, &broker, Action::Answer(0));
    let answered = ClientEvent::Answered {
        id: second.id,
        answer: "Yes".into(),
    };
    assert_eq!(events.try_recv().ok(), Some(answered));
}

fn press(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

fn char(c: char) -> Event {
    press(KeyCode::Char(c), KeyModifiers::NONE)
}

fn enter() -> Event {
    press(KeyCode::Enter, KeyModifiers::NONE)
}

#[test]
fn sends_a_typed_reply_without_acting_on_its_keys() {
    let (broker, client, mut events, _guard) = connected();
    let question = ask(client, &["Yes"]);
    broker.register(question.clone()).unwrap();
    let mut app = loaded(&broker);

    assert_eq!(on_event(&mut app, &broker, &char('r')), Step::Nothing);
    on_event(&mut app, &broker, &Event::Paste("  ".into()));
    on_event(&mut app, &broker, &enter());
    assert_eq!(
        app.reply().map(|r| r.1),
        Some("  "),
        "Enter on a blank reply keeps it open"
    );
    assert_eq!(app.notice(), None);
    assert!(events.try_recv().is_err());
    app.take_reply();
    on_event(&mut app, &broker, &char('r'));
    let keys = [
        char('1'),
        char('q'),
        char('d'),
        char('f'),
        press(KeyCode::Char('d'), KeyModifiers::CONTROL),
        press(KeyCode::Enter, KeyModifiers::ALT),
        Event::Paste(" ok\n".into()),
    ];
    for key in &keys {
        assert_eq!(on_event(&mut app, &broker, key), Step::Nothing);
    }
    assert!(events.try_recv().is_err(), "keys typed are not actions");
    assert_eq!(broker.snapshot().len(), 1);
    on_event(&mut app, &broker, &enter());
    let answered = ClientEvent::Answered {
        id: question.id,
        answer: "1qdf ok".into(),
    };
    assert_eq!(events.try_recv().ok(), Some(answered));
    assert_eq!(app.reply(), None);
    assert_eq!(
        app.answered_with(question.id),
        Some(&Choice::Reply("1qdf ok".into()))
    );
    assert_eq!(on_event(&mut app, &broker, &enter()), Step::Nothing);
    on_event(&mut app, &broker, &char('r'));
    assert_eq!(app.reply(), None, "no second reply while it is kept");

    on_event(&mut app, &broker, &char('k'));
    wait_out(&mut app, &broker);
    let quit = press(KeyCode::Char('c'), KeyModifiers::CONTROL);
    broker.register(ask(client, &["Yes"])).unwrap();
    app.set_questions(broker.snapshot());
    on_event(&mut app, &broker, &char('k'));
    on_event(&mut app, &broker, &char('r'));
    assert!(app.reply().is_some());
    assert_eq!(on_event(&mut app, &broker, &quit), Step::Quit);
}

#[test]
fn keeps_the_keys_out_of_commands_after_the_question_goes_away() {
    let (broker, client, mut events, _guard) = connected();
    let first = ask(client, &["Yes"]);
    let mut second = ask(client, &["Yes"]);
    second.asked_at = 2;
    let mut app = second_selected(&broker, &[first.clone(), second.clone()]);
    apply(&mut app, &broker, Action::Previous);

    on_event(&mut app, &broker, &char('r'));
    let ignored = [
        press(KeyCode::Down, KeyModifiers::NONE),
        press(KeyCode::Char('q'), KeyModifiers::ALT),
    ];
    app.set_notice("old");
    for key in &ignored {
        assert_eq!(on_event(&mut app, &broker, key), Step::Nothing);
    }
    assert_eq!(
        app.selected().map(|q| q.id),
        Some(first.id),
        "↓ while typing"
    );
    on_event(&mut app, &broker, &char('x'));
    assert_eq!(app.notice(), None, "a typed key clears the notice");
    broker.withdraw(&first.id, WithdrawReason::Deleted);
    assert!(events.try_recv().is_ok());
    app.set_questions(broker.snapshot());
    assert!(app.reply_gone());
    for key in [char('j'), char('d'), char('1'), char('q'), char('f')]
        .iter()
        .chain(&ignored)
    {
        assert_eq!(on_event(&mut app, &broker, key), Step::Nothing);
    }
    assert_eq!(app.selected().map(|q| q.id), Some(second.id));
    assert!(events.try_recv().is_err(), "nothing answered");
    assert_eq!(broker.snapshot(), vec![second]);
    broker.register(first.clone()).unwrap();
    app.set_questions(broker.snapshot());
    assert!(
        app.reply_gone(),
        "asked again, but another question is selected"
    );
    on_event(&mut app, &broker, &enter());
    assert_eq!(app.reply().map(|r| r.1), Some("x"), "Enter keeps the text");
    assert!(events.try_recv().is_err(), "nothing sent");
    on_event(&mut app, &broker, &press(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.reply(), None, "Esc closes the box");
    assert_eq!(app.notice(), None);
}

#[test]
fn cancels_a_reply_and_tells_when_it_cannot_be_sent() {
    let (broker, client, events, _guard) = connected();
    broker.register(ask(client, &["Yes"])).unwrap();
    let mut app = App::default();
    app.set_questions(broker.snapshot());

    apply(&mut app, &broker, Action::Reply);
    type_into(&mut app, &broker, Typing::Text("draft".into()));
    type_into(&mut app, &broker, Typing::Cancel);
    assert_eq!(app.reply(), None);
    assert!(
        !app.just_moved(),
        "closing a box with its question is no move"
    );
    assert_eq!(broker.snapshot().len(), 1, "nothing sent");

    drop(events);
    apply(&mut app, &broker, Action::Reply);
    type_into(&mut app, &broker, Typing::Text("late".into()));
    type_into(&mut app, &broker, Typing::Send);
    assert!(app.notice().unwrap().starts_with("not sent"));
    assert_eq!(app.reply(), None);
    assert!(app.selected().is_none(), "not kept as answered");
}

#[test]
fn keeps_nothing_when_the_delay_is_zero() {
    let (broker, client, _events, _guard) = connected();
    broker.register(ask(client, &["Yes"])).unwrap();
    let mut app = App::default();
    app.set_answered_for(Duration::ZERO);
    app.set_questions(broker.snapshot());
    app.set_shown_buttons(9);
    apply(&mut app, &broker, Action::Answer(0));
    assert!(app.selected().is_none());

    broker.register(ask(client, &["Yes"])).unwrap();
    app.set_questions(broker.snapshot());
    apply(&mut app, &broker, Action::Previous);
    on_event(&mut app, &broker, &char('r'));
    on_event(&mut app, &broker, &char('y'));
    on_event(&mut app, &broker, &enter());
    assert!(app.selected().is_none());
}

#[test]
fn lets_another_question_be_answered_while_one_is_kept() {
    let (broker, client, mut events, _guard) = connected();
    let mut questions: Vec<_> = (1..=3).map(|_| ask(client, &["Yes"])).collect();
    for (at, q) in (1..).zip(questions.iter_mut()) {
        q.asked_at = at;
    }
    let mut app = second_selected(&broker, &questions);

    apply(&mut app, &broker, Action::Answer(0));
    assert!(events.try_recv().is_ok());
    let mut late = ask(client, &["Yes"]);
    late.asked_at = 4;
    broker.register(late.clone()).unwrap();
    app.set_questions(broker.snapshot());
    app.select_next();
    assert_eq!(
        app.selected().map(|q| q.id),
        Some(questions[2].id),
        "kept in its place"
    );
    assert_eq!(app.answered_with(questions[2].id), None);

    apply(&mut app, &broker, Action::Answer(0));
    let answered = ClientEvent::Answered {
        id: questions[2].id,
        answer: "Yes".into(),
    };
    assert_eq!(events.try_recv().ok(), Some(answered));
    assert_eq!(app.answered_with(questions[1].id), None, "replaced");
    assert!(!app.just_moved());

    app.select_next();
    assert_eq!(app.selected().map(|q| q.id), Some(late.id));
    wait_out(&mut app, &broker);
    assert_eq!(
        app.selected().map(|q| q.id),
        Some(late.id),
        "a choice stays"
    );
    assert!(!app.just_moved());
}

#[test]
fn answers_the_selected_question_with_the_original_option() {
    let (broker, client, mut events, _guard) = connected();
    let first = ask(client, &["Yes"]);
    let mut second = ask(client, &["Yes", "No\u{1b}[2J"]);
    second.asked_at = 2;
    let mut app = second_selected(&broker, &[first.clone(), second.clone()]);

    apply(&mut app, &broker, Action::Answer(2));
    assert!(events.try_recv().is_err(), "no third button");
    app.set_shown_buttons(1);
    apply(&mut app, &broker, Action::Answer(1));
    assert!(events.try_recv().is_err(), "the second button is hidden");
    app.set_shown_buttons(2);
    apply(&mut app, &broker, Action::Answer(1));
    let answered = ClientEvent::Answered {
        id: second.id,
        answer: "No\u{1b}[2J".into(),
    };
    assert_eq!(events.try_recv().unwrap(), answered);
    assert_eq!(broker.snapshot(), vec![first]);
    assert!(app.notice().is_none());
}

#[test]
fn deletes_the_selected_question() {
    let (broker, client, mut events, _guard) = connected();
    let first = ask(client, &[]);
    let mut second = ask(client, &[]);
    second.asked_at = 2;
    let mut app = second_selected(&broker, &[first.clone(), second.clone()]);

    apply(&mut app, &broker, Action::Delete);
    let withdrawn = ClientEvent::Withdrawn {
        id: second.id,
        reason: WithdrawReason::Deleted,
    };
    assert_eq!(events.try_recv().unwrap(), withdrawn);
    assert_eq!(broker.snapshot(), vec![first.clone()]);
    apply(&mut app, &broker, Action::Delete);
    assert!(events.try_recv().is_err(), "a held d stops at the next one");
    assert_eq!(broker.snapshot(), vec![first.clone()]);
    app.settle_move();
    apply(&mut app, &broker, Action::Delete);
    assert!(broker.snapshot().is_empty());
}

#[test]
fn keeps_ignoring_a_key_held_past_the_guard() {
    let (broker, client, mut events, _guard) = connected();
    let first = ask(client, &["Yes"]);
    let mut next = ask(client, &["Allow"]);
    next.asked_at = 2;
    for q in [&first, &next] {
        broker.register(q.clone()).unwrap();
    }
    let mut app = loaded(&broker);

    answer_and_wait_out(&mut app, &broker, &mut events);
    app.age_move(MOVE_GUARD / 2 + Duration::from_millis(100));
    apply(&mut app, &broker, Action::Answer(0));
    app.age_move(MOVE_GUARD / 2 + Duration::from_millis(100));
    assert!(app.just_moved(), "a repeat restarts the guard");
    apply(&mut app, &broker, Action::Answer(0));
    assert!(events.try_recv().is_err());
}

#[test]
fn ignores_held_keys_just_after_the_selection_moved() {
    let (broker, client, mut events, _guard) = connected();
    let first = ask(client, &["Yes"]);
    let mut next = ask(client, &["Allow"]);
    next.asked_at = 2;
    for q in [&first, &next] {
        broker.register(q.clone()).unwrap();
    }
    let mut app = loaded(&broker);

    answer_and_wait_out(&mut app, &broker, &mut events);
    for action in [
        Action::Answer(0),
        Action::Answer(0),
        Action::Delete,
        Action::Reply,
    ] {
        apply(&mut app, &broker, action);
        assert!(
            events.try_recv().is_err(),
            "nothing sent to the next question"
        );
        assert_eq!(app.notice(), None);
    }
    assert!(app.reply().is_none());
    assert_eq!(broker.snapshot(), vec![next.clone()], "not deleted");
    app.settle_move();
    apply(&mut app, &broker, Action::Answer(0));
    let answered = ClientEvent::Answered {
        id: next.id,
        answer: "Allow".into(),
    };
    assert_eq!(events.try_recv().ok(), Some(answered));
}

#[test]
fn answers_at_once_a_question_that_arrives_after_the_guard() {
    let (broker, client, mut events, _guard) = connected();
    broker.register(ask(client, &["Yes"])).unwrap();
    let mut app = loaded(&broker);

    answer_and_wait_out(&mut app, &broker, &mut events);
    app.settle_move();
    let next = ask(client, &["Allow"]);
    broker.register(next.clone()).unwrap();
    app.set_questions(broker.snapshot());
    apply(&mut app, &broker, Action::Answer(0));
    let answered = ClientEvent::Answered {
        id: next.id,
        answer: "Allow".into(),
    };
    assert_eq!(events.try_recv().ok(), Some(answered));
}

#[test]
fn answers_a_question_that_arrives_after_the_list_emptied() {
    let (broker, client, mut events, _guard) = connected();
    let first = ask(client, &["Yes"]);
    broker.register(first).unwrap();
    let mut app = loaded(&broker);

    answer_and_wait_out(&mut app, &broker, &mut events);
    app.age_move(MOVE_GUARD / 2 + Duration::from_millis(100));
    apply(&mut app, &broker, Action::Answer(0));
    app.age_move(MOVE_GUARD / 2 + Duration::from_millis(100));
    let next = ask(client, &["Allow"]);
    broker.register(next.clone()).unwrap();
    app.set_questions(broker.snapshot());
    apply(&mut app, &broker, Action::Answer(0));
    assert!(
        events.try_recv().is_err(),
        "a key held over the empty list keeps the guard"
    );
    app.settle_move();
    apply(&mut app, &broker, Action::Answer(0));
    let answered = ClientEvent::Answered {
        id: next.id,
        answer: "Allow".into(),
    };
    assert_eq!(events.try_recv().ok(), Some(answered));
    assert_eq!(app.notice(), None);
}

#[test]
fn tells_when_the_answer_could_not_be_delivered_until_the_next_key() {
    let (broker, client, events, _guard) = connected();
    drop(events);
    broker.register(ask(client, &["Yes"])).unwrap();
    let mut app = loaded(&broker);

    apply(&mut app, &broker, Action::Answer(0));
    assert!(app.notice().unwrap().contains("ask again"));
    assert!(app.selected().is_none(), "not kept as answered");
    apply(&mut app, &broker, Action::Next);
    assert!(app.notice().is_none());
}

#[test]
fn focuses_the_selected_pane_even_just_after_answering() {
    let (broker, client, _events, _guard) = connected();
    let mut app = App::default();
    assert_eq!(
        apply(&mut app, &broker, Action::Focus),
        None,
        "no questions"
    );

    let first = ask(client, &["Yes"]);
    let mut next = ask(client, &["Yes"]);
    next.asked_at = 2;
    next.pane_id = PaneId("w1:p2".into());
    let mut app = second_selected(&broker, &[first.clone(), next.clone()]);
    let target = Some(Effect::Focus(next.id, next.pane_id.clone()));
    assert_eq!(apply(&mut app, &broker, Action::Focus), target);
    apply(&mut app, &broker, Action::Answer(0));
    assert_eq!(apply(&mut app, &broker, Action::Focus), target, "answered");

    wait_out(&mut app, &broker);
    assert!(app.just_moved());
    assert_eq!(apply(&mut app, &broker, Action::Focus), None, "just moved");
    app.settle_move();
    let moved = Some(Effect::Focus(first.id, first.pane_id.clone()));
    assert_eq!(apply(&mut app, &broker, Action::Focus), moved, "moved");
}

#[test]
fn withdraws_only_a_question_whose_pane_was_focused() {
    let broker = Broker::new();
    let (asker, other) = (ClientId(Uuid::new_v4()), ClientId(Uuid::new_v4()));
    let (mut asked, _guard) = broker.connect(asker);
    let (mut untouched, _other_guard) = broker.connect(other);
    let question = test_support::permission().client(asker);
    let mut rest = ask(other, &["Yes"]);
    rest.asked_at = 2;
    broker.register(question.clone()).unwrap();
    broker.register(rest.clone()).unwrap();
    let mut app = App::default();
    app.set_questions(broker.snapshot());

    app.set_notice("not answered: later");
    after_focus(&mut app, &broker, Err("not focused on w1:p1: x".into()));
    assert_eq!(app.notice(), Some("not answered: later"), "kept");
    assert_eq!(broker.snapshot().len(), 2, "kept without a focus");

    app.clear_notice();
    after_focus(&mut app, &broker, Err("not focused on w1:p1: x".into()));
    assert_eq!(app.notice(), Some("not focused on w1:p1: x"));
    assert!(!app.just_moved());
    after_focus(&mut app, &broker, Ok(question.id));
    assert!(app.notice().is_some(), "a success leaves the notice");
    assert!(app.just_moved());
    assert_eq!(
        asked.try_recv().unwrap(),
        ClientEvent::Withdrawn {
            id: question.id,
            reason: WithdrawReason::AnsweredInPane,
        }
    );
    assert_eq!(broker.snapshot(), vec![rest.clone()]);
    assert!(untouched.try_recv().is_err(), "the other question stays");
    assert_eq!(app.selected(), Some(&rest));
    assert_eq!(
        apply(&mut app, &broker, Action::Focus),
        None,
        "a repeated f stays off the next question"
    );

    after_focus(&mut app, &broker, Ok(rest.id));
    assert_eq!(
        untouched.try_recv().unwrap(),
        ClientEvent::Withdrawn {
            id: rest.id,
            reason: WithdrawReason::AnsweredInPane,
        },
        "ask questions leave too"
    );
    assert!(app.selected().is_none());
}

fn with_links(client: ClientId, links: &[&str]) -> Question {
    let mut question = ask(client, &["Yes"]);
    if let QuestionBody::Ask { links: l, .. } = &mut question.body {
        *l = links.iter().map(|l| l.to_string()).collect();
    }
    question
}

#[test]
fn opens_the_link_after_o_and_never_answers_with_that_digit() {
    let (broker, client, mut events, _guard) = connected();
    let mut question = with_links(client, &["https://a.example/", "https://b.example/"]);
    if let QuestionBody::Ask { options, .. } = &mut question.body {
        options.push("No".into());
    }
    broker.register(question).unwrap();
    let mut app = App::default();
    app.set_questions(broker.snapshot());
    app.set_shown_buttons(2);
    app.set_shown_links(2);

    let open = |url: &str| Some(Effect::Open(url.into()));
    apply(&mut app, &broker, Action::Open);
    assert_eq!(
        apply(&mut app, &broker, Action::Answer(1)),
        open("https://b.example/")
    );
    assert!(!app.opening(), "opening a link ends the wait");
    assert_eq!(
        apply(&mut app, &broker, Action::Answer(1)),
        None,
        "held down"
    );
    assert!(events.try_recv().is_err(), "the held digit does not answer");
    app.settle_move();
    apply(&mut app, &broker, Action::Open);
    assert_eq!(
        apply(&mut app, &broker, Action::Open),
        None,
        "o keeps waiting"
    );
    assert_eq!(
        apply(&mut app, &broker, Action::Answer(0)),
        open("https://a.example/")
    );
    apply(&mut app, &broker, Action::Open);
    app.set_shown_links(1);
    assert_eq!(
        apply(&mut app, &broker, Action::Answer(1)),
        None,
        "hidden link"
    );
    app.set_shown_links(0);
    assert_eq!(
        apply(&mut app, &broker, Action::Answer(0)),
        None,
        "nothing on screen"
    );
    assert!(app.opening(), "digits keep waiting");
    assert!(events.try_recv().is_err(), "no digit answered");

    let selected = app.selected_index();
    apply(&mut app, &broker, Action::Next);
    assert!(!app.opening(), "another key ends the wait");
    assert_eq!(app.selected_index(), selected, "and does nothing else");
    app.set_shown_links(2);
    apply(&mut app, &broker, Action::Open);
    assert_eq!(on_event(&mut app, &broker, &enter()), Step::Nothing);
    assert!(!app.opening(), "Enter only cancels");
    apply(&mut app, &broker, Action::Open);
    assert_eq!(apply(&mut app, &broker, Action::Delete), None);
    assert_eq!(broker.snapshot().len(), 1, "d only cancels");
    assert!(!app.opening());
    apply(&mut app, &broker, Action::Open);
    assert_eq!(apply(&mut app, &broker, Action::Focus), None, "f cancels o");
    assert!(!app.opening());

    app.set_shown_links(0);
    apply(&mut app, &broker, Action::Open);
    assert!(!app.opening(), "nothing to open");

    app.settle_move();
    app.set_shown_links(2);
    apply(&mut app, &broker, Action::Open);
    assert_eq!(
        apply(&mut app, &broker, Action::Answer(1)),
        open("https://b.example/")
    );
    assert_eq!(apply(&mut app, &broker, Action::Answer(1)), None);
    assert!(events.try_recv().is_err(), "held after opening");
    app.settle_move();
    apply(&mut app, &broker, Action::Answer(1));
    let answered = events.try_recv().unwrap();
    assert!(
        matches!(&answered, ClientEvent::Answered { answer, .. } if answer == "No"),
        "a digit answers once the guard is over: {answered:?}"
    );
}

#[test]
fn ends_the_wait_for_a_link_when_the_selection_moves() {
    let (broker, client, _events, _guard) = connected();
    let mut app = App::default();
    app.set_shown_links(1);
    apply(&mut app, &broker, Action::Open);
    assert!(!app.opening(), "no questions");

    let first = with_links(client, &["https://a.example/"]);
    let mut next = with_links(client, &["https://b.example/"]);
    next.asked_at = 2;
    let mut app = second_selected(&broker, &[first.clone(), next]);
    app.set_shown_links(1);
    apply(&mut app, &broker, Action::Open);
    assert!(app.opening());
    let mut another = with_links(client, &[]);
    another.asked_at = 0;
    broker.register(another).unwrap();
    assert_eq!(app.selected_index(), Some(1));
    app.set_questions(broker.snapshot());
    assert_eq!(
        app.selected_index(),
        Some(2),
        "an older question came first"
    );
    assert!(app.opening(), "a new question does not end the wait");
    let opened = apply(&mut app, &broker, Action::Answer(0));
    assert_eq!(opened, Some(Effect::Open("https://b.example/".into())));
    // 開く待ちのまま、選んでいた質問が取り下げられた
    broker.withdraw(&app.selected().unwrap().id, WithdrawReason::Deleted);
    app.set_questions(broker.snapshot());
    assert!(app.just_moved());
    assert!(!app.opening(), "the wait ends with the question");
    apply(&mut app, &broker, Action::Open);
    assert!(app.opening(), "links of the question it moved to");
    let opened = apply(&mut app, &broker, Action::Answer(0));
    assert_eq!(opened, Some(Effect::Open("https://a.example/".into())));
}

#[test]
fn opens_only_plain_web_links() {
    for link in ["https://a.example/x", "HTTP://A.example"] {
        assert!(checked_url(link).is_ok(), "{link}");
    }
    for link in [
        "file:///etc/passwd",
        "vscode://x",
        "ftp://x",
        "-a",
        "not a url",
        "https://github.com@evil.example/",
        "https://u:p@example.com/",
        "https://:p@example.com/",
    ] {
        assert!(checked_url(link).is_err(), "{link}");
    }
}

#[test]
fn answers_and_selects_by_click() {
    let (broker, client, mut events, _guard) = connected();
    let first = with_links(client, &["https://a.example/"]);
    let mut next = with_links(client, &["https://b.example/"]);
    next.asked_at = 2;
    let mut app = second_selected(&broker, &[first.clone(), next.clone()]);
    app.set_shown_links(1);

    let click = |target| Action::Click(target);
    let opened = apply(&mut app, &broker, click(Target::Link(0)));
    assert_eq!(opened, Some(Effect::Open("https://b.example/".into())));
    assert_eq!(
        apply(&mut app, &broker, click(Target::Link(1))),
        None,
        "hidden link"
    );

    apply(&mut app, &broker, Action::Open);
    apply(&mut app, &broker, click(Target::Button(0)));
    assert!(!app.opening(), "a click ends the wait");
    let answered = ClientEvent::Answered {
        id: next.id,
        answer: "Yes".into(),
    };
    assert_eq!(events.try_recv().unwrap(), answered, "and answers");

    wait_out(&mut app, &broker);
    assert!(app.just_moved());
    apply(&mut app, &broker, click(Target::Button(0)));
    assert!(
        events.try_recv().is_err(),
        "not just after the selection moved"
    );
    let link = apply(&mut app, &broker, click(Target::Link(0)));
    assert_eq!(link, Some(Effect::Open("https://a.example/".into())));
    apply(&mut app, &broker, click(Target::Question(first.id)));
    assert!(!app.just_moved(), "a click on the list is a choice");
    apply(&mut app, &broker, click(Target::Button(0)));
    assert!(events.try_recv().is_ok());
}
