use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use hailr_core::protocol::{ClientEvent, ClientId, Question, QuestionBody};
use uuid::Uuid;

use super::*;
use crate::broker::{AnswerError, ClientGuard};
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

/// 返事の枠に `1qdf ok` を打って Enter で送った状態にする
fn sent_typed_reply() -> (
    Arc<Broker>,
    mpsc::Receiver<ClientEvent>,
    ClientGuard,
    App,
    Question,
    ClientId,
) {
    let (broker, client, events, guard) = connected();
    let question = ask(client, &["Yes"]);
    broker.register(question.clone()).unwrap();
    let mut app = loaded(&broker);
    on_event(&mut app, &broker, &char('r'));
    for key in &typed_keys() {
        on_event(&mut app, &broker, key);
    }
    on_event(&mut app, &broker, &enter());
    assert!(app.answered_with(question.id).is_some());
    (broker, events, guard, app, question, client)
}

fn typed_keys() -> [Event; 7] {
    [
        char('1'),
        char('q'),
        char('d'),
        char('f'),
        press(KeyCode::Char('d'), KeyModifiers::CONTROL),
        press(KeyCode::Enter, KeyModifiers::ALT),
        Event::Paste(" ok\n".into()),
    ]
}

#[test]
fn keeps_a_blank_reply_open_on_enter() {
    let (broker, client, mut events, _guard) = connected();
    broker.register(ask(client, &["Yes"])).unwrap();
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
}

#[test]
fn types_keys_into_a_reply_without_acting_on_them() {
    let (broker, client, mut events, _guard) = connected();
    broker.register(ask(client, &["Yes"])).unwrap();
    let mut app = loaded(&broker);

    on_event(&mut app, &broker, &char('r'));
    for key in &typed_keys() {
        assert_eq!(on_event(&mut app, &broker, key), Step::Nothing);
    }
    assert!(events.try_recv().is_err(), "keys typed are not actions");
    assert_eq!(broker.snapshot().len(), 1);
    assert_eq!(app.reply().map(|r| r.1), Some("1qdf ok "));
}

#[test]
fn sends_the_typed_reply_on_enter() {
    let (_broker, mut events, _guard, app, question, _client) = sent_typed_reply();

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
}

#[test]
fn does_not_open_a_second_reply_while_the_sent_one_is_kept() {
    let (broker, _events, _guard, mut app, _question, _client) = sent_typed_reply();

    assert_eq!(on_event(&mut app, &broker, &enter()), Step::Nothing);
    on_event(&mut app, &broker, &char('r'));
    assert_eq!(app.reply(), None, "no second reply while it is kept");
}

#[test]
fn quits_on_ctrl_c_while_replying_to_a_question_asked_after_one_was_kept() {
    let (broker, _events, _guard, mut app, _question, client) = sent_typed_reply();

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

fn ignored_keys() -> [Event; 2] {
    [
        press(KeyCode::Down, KeyModifiers::NONE),
        press(KeyCode::Char('q'), KeyModifiers::ALT),
    ]
}

/// 1 件目への返事の枠に `x` を打った状態にする（2 件目も並んでいる）
fn typing_on_first() -> (
    Arc<Broker>,
    mpsc::Receiver<ClientEvent>,
    ClientGuard,
    App,
    Question,
    Question,
) {
    let (broker, client, events, guard) = connected();
    let first = ask(client, &["Yes"]);
    let mut second = ask(client, &["Yes"]);
    second.asked_at = 2;
    let mut app = second_selected(&broker, &[first.clone(), second.clone()]);
    apply(&mut app, &broker, Action::Previous);
    on_event(&mut app, &broker, &char('r'));
    on_event(&mut app, &broker, &char('x'));
    (broker, events, guard, app, first, second)
}

/// 返事を打っている 1 件目が取り下げられ、一覧を読み直した状態にする
fn first_withdrawn_while_typing() -> (
    Arc<Broker>,
    mpsc::Receiver<ClientEvent>,
    ClientGuard,
    App,
    Question,
    Question,
) {
    let (broker, mut events, guard, mut app, first, second) = typing_on_first();
    broker.withdraw(&first.id, WithdrawReason::Deleted);
    assert!(events.try_recv().is_ok());
    app.set_questions(broker.snapshot());
    (broker, events, guard, app, first, second)
}

#[test]
fn ignores_command_keys_and_clears_the_notice_while_typing() {
    let (broker, client, _events, _guard) = connected();
    let first = ask(client, &["Yes"]);
    let mut second = ask(client, &["Yes"]);
    second.asked_at = 2;
    let mut app = second_selected(&broker, &[first.clone(), second.clone()]);
    apply(&mut app, &broker, Action::Previous);

    on_event(&mut app, &broker, &char('r'));
    app.set_notice("old");
    for key in &ignored_keys() {
        assert_eq!(on_event(&mut app, &broker, key), Step::Nothing);
    }
    assert_eq!(
        app.selected().map(|q| q.id),
        Some(first.id),
        "↓ while typing"
    );
    on_event(&mut app, &broker, &char('x'));
    assert_eq!(app.notice(), None, "a typed key clears the notice");
}

#[test]
fn keeps_the_keys_out_of_commands_after_the_question_goes_away() {
    let (broker, mut events, _guard, mut app, _first, second) = first_withdrawn_while_typing();

    assert!(app.reply_gone());
    for key in [char('j'), char('d'), char('1'), char('q'), char('f')]
        .iter()
        .chain(&ignored_keys())
    {
        assert_eq!(on_event(&mut app, &broker, key), Step::Nothing);
    }
    assert_eq!(app.selected().map(|q| q.id), Some(second.id));
    assert!(events.try_recv().is_err(), "nothing answered");
    assert_eq!(broker.snapshot(), vec![second]);
    assert_eq!(
        app.reply().map(|r| r.1),
        Some("x"),
        "keys after it went away are not typed"
    );
}

#[test]
fn keeps_the_stopped_reply_until_esc_when_the_question_is_asked_again() {
    let (broker, mut events, _guard, mut app, first, _second) = first_withdrawn_while_typing();

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

/// 権限の質問と、別の client の ask の質問を並べ、どちらも取り下げを受け取れる状態にする
struct PermissionAndAsk {
    broker: Arc<Broker>,
    asked: mpsc::Receiver<ClientEvent>,
    untouched: mpsc::Receiver<ClientEvent>,
    question: Question,
    rest: Question,
    app: App,
    _guards: [ClientGuard; 2],
}

fn permission_and_ask() -> PermissionAndAsk {
    let broker = Broker::new();
    let (asker, other) = (ClientId(Uuid::new_v4()), ClientId(Uuid::new_v4()));
    let (asked, asker_guard) = broker.connect(asker);
    let (untouched, other_guard) = broker.connect(other);
    let question = test_support::permission().client(asker);
    let mut rest = ask(other, &["Yes"]);
    rest.asked_at = 2;
    broker.register(question.clone()).unwrap();
    broker.register(rest.clone()).unwrap();
    let mut app = App::default();
    app.set_questions(broker.snapshot());
    PermissionAndAsk {
        broker,
        asked,
        untouched,
        question,
        rest,
        app,
        _guards: [asker_guard, other_guard],
    }
}

#[test]
fn keeps_a_question_when_its_pane_was_not_focused() {
    let PermissionAndAsk {
        broker,
        asked: _asked,
        untouched: _untouched,
        mut app,
        _guards,
        ..
    } = permission_and_ask();

    app.set_notice("not answered: later");
    after_focus(&mut app, &broker, Err("not focused on w1:p1: x".into()));
    assert_eq!(app.notice(), Some("not answered: later"), "kept");
    assert_eq!(broker.snapshot().len(), 2, "kept without a focus");

    app.clear_notice();
    after_focus(&mut app, &broker, Err("not focused on w1:p1: x".into()));
    assert_eq!(app.notice(), Some("not focused on w1:p1: x"));
    assert!(!app.just_moved());
}

#[test]
fn withdraws_only_a_question_whose_pane_was_focused() {
    let PermissionAndAsk {
        broker,
        mut asked,
        mut untouched,
        question,
        rest,
        mut app,
        _guards,
    } = permission_and_ask();

    app.set_notice("not focused on w1:p1: x");
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

/// 2 件のリンクと 3 つ目のボタン（No）がある質問を、リンクとボタンを出した状態にする
fn with_links_shown() -> (Arc<Broker>, mpsc::Receiver<ClientEvent>, ClientGuard, App) {
    let (broker, client, events, guard) = connected();
    let mut question = with_links(client, &["https://a.example/", "https://b.example/"]);
    if let QuestionBody::Ask { options, .. } = &mut question.body {
        options.push("No".into());
    }
    broker.register(question).unwrap();
    let mut app = App::default();
    app.set_questions(broker.snapshot());
    app.set_shown_buttons(2);
    app.set_shown_links(2);
    (broker, events, guard, app)
}

fn open(url: &str) -> Option<Effect> {
    Some(Effect::Open(url.into()))
}

#[test]
fn opens_the_link_after_o_and_ignores_the_held_digit() {
    let (broker, mut events, _guard, mut app) = with_links_shown();

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
}

#[test]
fn keeps_waiting_for_a_link_on_o_and_opens_the_one_pressed() {
    let (broker, _events, _guard, mut app) = with_links_shown();

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
}

#[test]
fn keeps_waiting_for_a_link_on_digits_that_open_nothing() {
    let (broker, mut events, _guard, mut app) = with_links_shown();

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
}

#[test]
fn ends_the_wait_for_a_link_on_another_key_and_does_nothing_else() {
    let (broker, _events, _guard, mut app) = with_links_shown();

    apply(&mut app, &broker, Action::Open);
    let selected = app.selected_index();
    apply(&mut app, &broker, Action::Next);
    assert!(!app.opening(), "another key ends the wait");
    assert_eq!(app.selected_index(), selected, "and does nothing else");
}

#[test]
fn only_cancels_the_wait_for_a_link_on_enter() {
    let (broker, _events, _guard, mut app) = with_links_shown();

    apply(&mut app, &broker, Action::Open);
    assert_eq!(on_event(&mut app, &broker, &enter()), Step::Nothing);
    assert!(!app.opening(), "Enter only cancels");
}

#[test]
fn only_cancels_the_wait_for_a_link_on_d() {
    let (broker, _events, _guard, mut app) = with_links_shown();

    apply(&mut app, &broker, Action::Open);
    assert_eq!(apply(&mut app, &broker, Action::Delete), None);
    assert_eq!(broker.snapshot().len(), 1, "d only cancels");
    assert!(!app.opening());
}

#[test]
fn only_cancels_the_wait_for_a_link_on_f() {
    let (broker, _events, _guard, mut app) = with_links_shown();

    apply(&mut app, &broker, Action::Open);
    assert_eq!(apply(&mut app, &broker, Action::Focus), None, "f cancels o");
    assert!(!app.opening());
}

#[test]
fn does_not_wait_for_a_link_when_there_is_none_to_open() {
    let (broker, _events, _guard, mut app) = with_links_shown();

    app.set_shown_links(0);
    apply(&mut app, &broker, Action::Open);
    assert!(!app.opening(), "nothing to open");
}

#[test]
fn answers_with_the_digit_once_the_guard_after_opening_a_link_is_over() {
    let (broker, mut events, _guard, mut app) = with_links_shown();

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

#[test]
fn does_not_answer_a_button_that_an_update_removed() {
    let (broker, client, mut events, _guard) = connected();
    let question = ask(client, &["Yes", "No", "Maybe"]);
    broker.register(question.clone()).unwrap();
    let mut app = loaded(&broker);
    app.set_shown_buttons(3);

    let updated = question.options(&["Yes", "No"]);
    broker.register(updated.clone()).unwrap();
    app.set_questions(broker.snapshot());
    apply(&mut app, &broker, Action::Answer(2));
    assert!(events.try_recv().is_err(), "the third button is gone");
    assert!(app.notice().is_none());
    assert_eq!(broker.snapshot(), vec![updated.clone()]);
    apply(&mut app, &broker, Action::Answer(1));
    let answered = ClientEvent::Answered {
        id: updated.id,
        answer: "No".into(),
    };
    assert_eq!(
        events.try_recv().unwrap(),
        answered,
        "the buttons left still answer"
    );
}

#[test]
fn answers_with_the_reordered_options_after_an_update() {
    let (broker, client, mut events, _guard) = connected();
    let question = ask(client, &["Yes", "No"]);
    broker.register(question.clone()).unwrap();
    let mut app = loaded(&broker);

    let updated = question.clone().options(&["No", "Yes"]);
    broker.register(updated).unwrap();
    app.set_questions(broker.snapshot());
    apply(&mut app, &broker, Action::Answer(0));
    let answered = ClientEvent::Answered {
        id: question.id,
        answer: "No".into(),
    };
    assert_eq!(events.try_recv().unwrap(), answered);
    assert!(app.notice().is_none());
}

#[test]
fn refuses_an_option_removed_before_the_screen_caught_up() {
    let (broker, client, mut events, _guard) = connected();
    let question = ask(client, &["Yes", "No"]);
    broker.register(question.clone()).unwrap();
    let mut app = loaded(&broker);

    let updated = question.options(&["No"]);
    broker.register(updated.clone()).unwrap();
    apply(&mut app, &broker, Action::Answer(0));
    assert!(events.try_recv().is_err(), "the old first option is gone");
    assert!(
        app.notice()
            .unwrap()
            .ends_with(&AnswerError::NotAnOption.to_string())
    );
    assert_eq!(broker.snapshot(), vec![updated.clone()]);
    assert_eq!(app.selected(), Some(&updated));
}

/// 1 つ目の pane に 2 件、2 つ目の pane に 1 件の質問を並べ、1 件目を選んで画面を読んだ状態
struct Previewing {
    app: App,
    first: Question,
    same_pane: Question,
    other_pane: Question,
    pane_tx: watch::Sender<Option<PaneId>>,
    pane_rx: watch::Receiver<Option<PaneId>>,
}

fn previewing_first() -> Previewing {
    let client = ClientId(Uuid::new_v4());
    let first = ask(client, &["Yes"]).pane("w1:p1");
    let mut same_pane = ask(client, &["Yes"]).pane("w1:p1");
    same_pane.asked_at = 2;
    let mut other_pane = ask(client, &["Yes"]).pane("w2:p1");
    other_pane.asked_at = 3;
    let mut app = App::default();
    app.set_questions(vec![first.clone(), same_pane.clone(), other_pane.clone()]);
    let (pane_tx, pane_rx) = watch::channel(app.selected_pane());
    assert!(!pane_rx.has_changed().unwrap());
    app.set_shot((
        first.pane_id.clone(),
        Ok(ratatui::text::Text::raw("screen")),
    ));
    assert!(app.shot().is_some());
    Previewing {
        app,
        first,
        same_pane,
        other_pane,
        pane_tx,
        pane_rx,
    }
}

#[test]
fn repoints_the_preview_and_forgets_the_shot_when_the_pane_changes() {
    let Previewing {
        mut app,
        first,
        other_pane,
        pane_tx,
        mut pane_rx,
        ..
    } = previewing_first();

    app.select(other_pane.id);
    repoint(&mut app, &pane_tx);
    assert!(pane_rx.has_changed().unwrap());
    assert_eq!(*pane_rx.borrow_and_update(), Some(other_pane.pane_id));
    app.select(first.id);
    assert!(app.shot().is_none(), "the old screen is not shown again");
}

#[test]
fn keeps_the_preview_for_another_question_in_the_same_pane() {
    let Previewing {
        mut app,
        same_pane,
        pane_tx,
        pane_rx,
        ..
    } = previewing_first();

    app.select(same_pane.id);
    repoint(&mut app, &pane_tx);
    assert!(
        !pane_rx.has_changed().unwrap(),
        "the same pane is not read again"
    );
    assert!(app.shot().is_some());
}

#[test]
fn stops_the_preview_when_no_question_is_left() {
    let Previewing {
        mut app,
        pane_tx,
        mut pane_rx,
        ..
    } = previewing_first();

    app.set_questions(Vec::new());
    repoint(&mut app, &pane_tx);
    assert!(pane_rx.has_changed().unwrap());
    assert_eq!(*pane_rx.borrow_and_update(), None);
}
