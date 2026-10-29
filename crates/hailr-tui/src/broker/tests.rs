use hailr_core::protocol::PaneId;
use uuid::Uuid;

use super::*;
use crate::test_support::{self, QuestionExt};

fn client() -> ClientId {
    ClientId(Uuid::new_v4())
}

fn ask(client: ClientId, asked_at: u64, options: &[&str]) -> Question {
    test_support::ask()
        .client(client)
        .asked_at(asked_at)
        .options(options)
}

fn permission(client: ClientId, asked_at: u64) -> Question {
    test_support::permission().client(client).asked_at(asked_at)
}

#[test]
fn registers_new_then_updates_the_same_id() {
    let broker = Broker::new();
    let c = client();
    let (_rx, _guard) = broker.connect(c);
    let mut q = ask(c, 1, &[]);
    assert_eq!(broker.register(q.clone()), Ok(Registered::New));
    q.body = ask(c, 1, &["Yes"]).body;
    assert_eq!(broker.register(q.clone()), Ok(Registered::Updated));
    assert_eq!(broker.snapshot(), vec![q]);
}

#[test]
fn rejects_questions_from_unconnected_clients() {
    let broker = Broker::new();
    assert_eq!(
        broker.register(ask(client(), 1, &[])),
        Err(RegisterError::NoClient)
    );
}

#[test]
fn rejects_taking_over_another_clients_question() {
    let broker = Broker::new();
    let (a, b) = (client(), client());
    let (_ra, _ga) = broker.connect(a);
    let (_rb, _gb) = broker.connect(b);
    let q = ask(a, 1, &[]);
    broker.register(q.clone()).unwrap();
    let stolen = Question { client_id: b, ..q };
    assert_eq!(broker.register(stolen), Err(RegisterError::NotOwner));
}

#[test]
fn validates_input() {
    let broker = Broker::new();
    let c = client();
    let (_rx, _guard) = broker.connect(c);

    for pane_id in ["", "--help"] {
        let mut q = ask(c, 1, &[]);
        q.pane_id = PaneId(pane_id.into());
        assert!(
            matches!(broker.register(q), Err(RegisterError::Invalid(_))),
            "{pane_id:?}"
        );
    }

    let labels: Vec<String> = (0..=MAX_OPTIONS).map(|i| i.to_string()).collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    assert!(broker.register(ask(c, 1, &labels[..MAX_OPTIONS])).is_ok());
    for options in [&labels[..], &["Yes", "Yes"][..], &["Yes", " "][..]] {
        let result = broker.register(ask(c, 1, options));
        assert!(
            matches!(result, Err(RegisterError::Invalid(_))),
            "{options:?}"
        );
    }

    for (links, ok) in [
        (vec!["https://example.com"; MAX_LINKS], true),
        (vec!["HTTPS://github.com/mosmos21/hailr/pull/1"], true),
        (vec!["https://example.com"; MAX_LINKS + 1], false),
        (vec!["javascript:alert(1)"], false),
        (vec!["https://"], false),
        (vec!["https://example.com/a b"], false),
        (vec!["https://example.com/\n"], false),
    ] {
        let mut q = ask(c, 1, &[]);
        if let QuestionBody::Ask { links: l, .. } = &mut q.body {
            *l = links.iter().map(|s| s.to_string()).collect();
        }
        assert_eq!(broker.register(q).is_ok(), ok, "{links:?}");
    }
}

#[test]
fn snapshot_is_oldest_first() {
    let broker = Broker::new();
    let c = client();
    let (_rx, _guard) = broker.connect(c);
    for asked_at in [3, 1, 2] {
        broker.register(ask(c, asked_at, &[])).unwrap();
    }
    let order: Vec<_> = broker.snapshot().iter().map(|q| q.asked_at).collect();
    assert_eq!(order, [1, 2, 3]);
}

#[test]
fn answer_sends_the_option_and_removes_the_question() {
    let broker = Broker::new();
    let c = client();
    let (mut rx, _guard) = broker.connect(c);
    let q = ask(c, 1, &["Yes", "No"]);
    broker.register(q.clone()).unwrap();

    assert_eq!(
        broker.answer(&q.id, "Maybe".into()),
        Err(AnswerError::NotAnOption)
    );
    broker.answer(&q.id, "No".into()).unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        ClientEvent::Answered {
            id: q.id,
            answer: "No".into()
        }
    );
    assert!(broker.snapshot().is_empty());
    assert_eq!(
        broker.answer(&q.id, "No".into()),
        Err(AnswerError::NotFound)
    );
}

#[test]
fn withdraw_tells_the_owner() {
    let broker = Broker::new();
    let c = client();
    let (mut rx, _guard) = broker.connect(c);
    let q = ask(c, 1, &[]);
    broker.register(q.clone()).unwrap();
    broker.withdraw(&q.id, WithdrawReason::Deleted);
    assert_eq!(
        rx.try_recv().unwrap(),
        ClientEvent::Withdrawn {
            id: q.id,
            reason: WithdrawReason::Deleted
        }
    );
    assert!(broker.snapshot().is_empty());
}

#[test]
fn disconnect_drops_that_clients_questions_only() {
    let broker = Broker::new();
    let (a, b) = (client(), client());
    let (_ra, guard_a) = broker.connect(a);
    let (_rb, _gb) = broker.connect(b);
    broker.register(ask(a, 1, &[])).unwrap();
    let kept = ask(b, 2, &[]);
    broker.register(kept.clone()).unwrap();

    drop(guard_a);
    assert_eq!(broker.snapshot(), vec![kept]);
    assert_eq!(
        broker.register(ask(a, 3, &[])),
        Err(RegisterError::NoClient)
    );
}

#[test]
fn a_stale_disconnect_does_not_drop_the_new_connection() {
    let broker = Broker::new();
    let c = client();
    let (_old_rx, old_guard) = broker.connect(c);
    let (_new_rx, _new_guard) = broker.connect(c);
    let q = ask(c, 1, &[]);
    broker.register(q.clone()).unwrap();

    drop(old_guard);
    assert_eq!(broker.snapshot(), vec![q]);
}

#[test]
fn answers_permission_questions_with_allow_or_deny() {
    let broker = Broker::new();
    let c = client();
    let (mut rx, _guard) = broker.connect(c);
    let q = permission(c, 1);
    broker.register(q.clone()).unwrap();
    assert_eq!(
        broker.answer(&q.id, "Yes".into()),
        Err(AnswerError::NotAnOption)
    );
    broker.answer(&q.id, "Deny".into()).unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        ClientEvent::Answered {
            id: q.id,
            answer: "Deny".into()
        }
    );
}

#[test]
fn an_undeliverable_answer_disconnects_the_client() {
    let broker = Broker::new();
    let c = client();
    let (rx, _guard) = broker.connect(c);
    let q = ask(c, 1, &["Yes"]);
    let other = ask(c, 2, &[]);
    broker.register(q.clone()).unwrap();
    broker.register(other).unwrap();
    drop(rx);

    assert_eq!(
        broker.answer(&q.id, "Yes".into()),
        Err(AnswerError::Undelivered)
    );
    assert!(broker.snapshot().is_empty());
    assert_eq!(
        broker.register(ask(c, 3, &[])),
        Err(RegisterError::NoClient)
    );
}

#[test]
fn a_full_queue_disconnects_the_client_and_closes_its_stream() {
    let broker = Broker::new();
    let c = client();
    let (mut rx, _guard) = broker.connect(c);
    let questions: Vec<_> = (0..=CLIENT_QUEUE as u64).map(|i| ask(c, i, &[])).collect();
    for q in &questions {
        broker.register(q.clone()).unwrap();
    }
    for q in &questions {
        broker.withdraw(&q.id, WithdrawReason::Deleted);
    }
    assert!(broker.snapshot().is_empty());
    for _ in 0..CLIENT_QUEUE {
        assert!(rx.try_recv().is_ok());
    }
    // 送り手が手放されているので、溜まった分を読み切るとストリームが閉じる
    assert_eq!(rx.try_recv(), Err(mpsc::error::TryRecvError::Disconnected));
}

#[test]
fn reconnecting_drops_the_previous_copies() {
    let broker = Broker::new();
    let c = client();
    let (_old_rx, _old_guard) = broker.connect(c);
    broker.register(ask(c, 1, &[])).unwrap();
    let (mut new_rx, _new_guard) = broker.connect(c);
    assert!(broker.snapshot().is_empty());

    let q = ask(c, 2, &[]);
    broker.register(q.clone()).unwrap();
    broker.withdraw(&q.id, WithdrawReason::Deleted);
    assert!(new_rx.try_recv().is_ok());
}

#[test]
fn announces_a_question_once_even_across_reconnects() {
    let broker = Broker::new();
    let mut arrivals = broker.arrivals();
    let c = client();
    let (_old_rx, _old_guard) = broker.connect(c);
    let mut q = ask(c, 1, &[]);
    broker.register(q.clone()).unwrap();
    assert_eq!(arrivals.try_recv().unwrap(), q.id);

    q.body = ask(c, 1, &["Yes"]).body;
    broker.register(q.clone()).unwrap();
    let (_new_rx, _new_guard) = broker.connect(c);
    assert_eq!(broker.register(q.clone()), Ok(Registered::New));
    assert!(
        arrivals.try_recv().is_err(),
        "updates and re-registers stay quiet"
    );
}

#[test]
fn announces_again_once_a_question_is_settled() {
    let broker = Broker::new();
    let mut arrivals = broker.arrivals();
    let c = client();
    let (_rx, _guard) = broker.connect(c);
    type Settle = fn(&Broker, &Question);
    let settles: [(&str, Settle); 2] = [
        ("answer", |b, q| b.answer(&q.id, "Allow".into()).unwrap()),
        ("withdraw", |b, q| {
            b.withdraw(&q.id, WithdrawReason::Deleted)
        }),
    ];
    for (name, settle) in settles {
        let q = permission(c, 1);
        broker.register(q.clone()).unwrap();
        settle(&broker, &q);
        broker.register(q.clone()).unwrap();
        assert_eq!(arrivals.try_recv().unwrap(), q.id, "{name}");
        assert_eq!(arrivals.try_recv().unwrap(), q.id, "{name}");
        assert!(arrivals.try_recv().is_err(), "{name}");
        broker.withdraw(&q.id, WithdrawReason::Deleted);
    }
}

#[test]
fn announces_after_a_reconnect_only_if_the_notifier_missed_it() {
    let broker = Broker::new();
    let mut arrivals = broker.arrivals();
    let c = client();
    for missed in [false, true] {
        let q = permission(c, 1);
        let (_rx, guard) = broker.connect(c);
        broker.register(q.clone()).unwrap();
        if !missed {
            assert!(broker.to_announce(&q.id).is_some());
        }
        drop(guard);
        if missed {
            assert_eq!(broker.to_announce(&q.id), None);
        }
        let (_rx, _guard) = broker.connect(c);
        assert_eq!(broker.register(q.clone()), Ok(Registered::New));
        let announced = std::iter::from_fn(|| arrivals.try_recv().ok()).count();
        assert_eq!(announced, if missed { 2 } else { 1 }, "missed: {missed}");
    }
}

#[test]
fn reports_changes() {
    let broker = Broker::new();
    let changes = broker.changes();
    let advanced = |before: &mut u64| {
        let now = *changes.borrow();
        let moved = now > *before;
        *before = now;
        moved
    };
    let mut seen = *changes.borrow();
    let c = client();
    let (_rx, guard) = broker.connect(c);
    assert!(advanced(&mut seen), "connect");
    let q = ask(c, 1, &[]);
    broker.register(q.clone()).unwrap();
    assert!(advanced(&mut seen), "register");
    let answered = ask(c, 2, &["Yes"]);
    broker.register(answered.clone()).unwrap();
    assert!(advanced(&mut seen), "register another");
    broker.answer(&answered.id, "Yes".into()).unwrap();
    assert!(advanced(&mut seen), "answer");
    broker.withdraw(&q.id, WithdrawReason::Deleted);
    assert!(advanced(&mut seen), "withdraw");

    broker.withdraw(&q.id, WithdrawReason::Deleted);
    assert_eq!(
        broker.answer(&q.id, "Yes".into()),
        Err(AnswerError::NotFound)
    );
    assert!(!advanced(&mut seen), "no-ops must not notify");
    drop(guard);
    assert!(advanced(&mut seen), "disconnect");
}

#[test]
fn takes_a_typed_reply_for_questions_only() {
    let broker = Broker::new();
    let c = client();
    let (mut rx, _guard) = broker.connect(c);
    let q = ask(c, 1, &["Yes"]);
    broker.register(q.clone()).unwrap();
    assert_eq!(broker.reply(&q.id, " \t".into()), Err(AnswerError::Empty));
    let long = "あ".repeat(MAX_REPLY + 1);
    assert_eq!(broker.reply(&q.id, long), Err(AnswerError::TooLong));
    broker.reply(&q.id, "あ".repeat(MAX_REPLY)).unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        ClientEvent::Answered {
            id: q.id,
            answer: "あ".repeat(MAX_REPLY)
        }
    );
    assert_eq!(broker.snapshot(), vec![]);
    assert_eq!(broker.reply(&q.id, "x".into()), Err(AnswerError::NotFound));

    let permission = permission(c, 2);
    broker.register(permission.clone()).unwrap();
    assert_eq!(
        broker.reply(&permission.id, "Allow".into()),
        Err(AnswerError::NotAQuestion)
    );
}

#[test]
fn close_all_drops_everything_and_closes_the_streams() {
    let broker = Broker::new();
    let changes = broker.changes();
    let c = client();
    let (mut rx, _guard) = broker.connect(c);
    broker.register(ask(c, 1, &["Yes"])).unwrap();
    let before = *changes.borrow();

    broker.close_all();
    assert!(broker.snapshot().is_empty());
    assert_eq!(rx.try_recv(), Err(mpsc::error::TryRecvError::Disconnected));
    assert!(*changes.borrow() > before, "close_all must notify");
    assert_eq!(
        broker.register(ask(c, 2, &[])),
        Err(RegisterError::NoClient)
    );
}

#[test]
fn announces_again_after_close_all() {
    let broker = Broker::new();
    let mut arrivals = broker.arrivals();
    let c = client();
    let (_old_rx, _old_guard) = broker.connect(c);
    let q = permission(c, 1);
    broker.register(q.clone()).unwrap();
    assert_eq!(arrivals.try_recv().unwrap(), q.id);

    broker.close_all();
    let (_new_rx, _new_guard) = broker.connect(c);
    assert_eq!(broker.register(q.clone()), Ok(Registered::New));
    assert_eq!(arrivals.try_recv().unwrap(), q.id);
    assert!(arrivals.try_recv().is_err());
}
