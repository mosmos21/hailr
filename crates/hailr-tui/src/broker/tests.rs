use hailr_core::mux::{AgentStatus, PaneInfo};
use hailr_core::protocol::PaneId;
use uuid::Uuid;

use super::*;

fn client() -> ClientId {
    ClientId(Uuid::new_v4())
}

fn ask(client: ClientId, asked_at: u64, options: &[&str]) -> Question {
    Question {
        id: QuestionId(Uuid::new_v4()),
        client_id: client,
        pane_id: PaneId("w1:p1".into()),
        asked_at,
        body: QuestionBody::Ask {
            title: "t".into(),
            question: "q".into(),
            options: options.iter().map(|o| o.to_string()).collect(),
            links: vec![],
        },
    }
}

fn pane(id: &str, status: AgentStatus) -> PaneInfo {
    PaneInfo {
        id: PaneId(id.into()),
        status,
        label: id.into(),
    }
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
    let mut q = ask(c, 1, &[]);
    q.body = QuestionBody::Permission {
        request_id: "abcde".into(),
        tool_name: "Bash".into(),
        description: "d".into(),
        input_preview: "p".into(),
    };
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

/// 一覧を取り始めてから終わるまでを 1 回ぶん流す
fn poll(broker: &Broker, status: AgentStatus) {
    broker.reconcile_panes(broker.listing_start(), &[pane("w1:p1", status)]);
}

#[test]
fn reconcile_keeps_questions_of_unlisted_and_unattached_panes() {
    let broker = Broker::new();
    let c = client();
    let (mut rx, _guard) = broker.connect(c);
    let mut unlisted = ask(c, 1, &[]);
    unlisted.pane_id = PaneId("w1:p9".into());
    let unattached = ask(c, 2, &[]);
    broker.register(unlisted.clone()).unwrap();
    broker.register(unattached.clone()).unwrap();
    broker.mark_attached(&unlisted.id);

    for status in [AgentStatus::Idle, AgentStatus::Working] {
        poll(&broker, status);
    }
    assert!(rx.try_recv().is_err());
    assert_eq!(broker.snapshot(), vec![unlisted, unattached]);
}

#[test]
fn reconcile_withdraws_once_the_attached_pane_waits_then_works() {
    for (waiting, arms) in [
        (AgentStatus::Idle, true),
        (AgentStatus::Blocked, true),
        (AgentStatus::Other("unknown".into()), false),
        (AgentStatus::Working, false),
    ] {
        let broker = Broker::new();
        let c = client();
        let (mut rx, _guard) = broker.connect(c);
        let q = ask(c, 1, &[]);
        broker.register(q.clone()).unwrap();
        broker.mark_attached(&q.id);

        poll(&broker, waiting.clone());
        poll(&broker, AgentStatus::Working);
        if arms {
            assert_eq!(
                rx.try_recv().unwrap(),
                ClientEvent::Withdrawn {
                    id: q.id,
                    reason: WithdrawReason::AnsweredInPane
                },
                "{waiting:?}"
            );
        } else {
            assert_eq!(broker.snapshot(), vec![q], "{waiting:?}");
        }
    }
}

#[test]
fn reconcile_ignores_a_listing_started_before_the_attach() {
    let broker = Broker::new();
    let c = client();
    let (_rx, _guard) = broker.connect(c);
    let q = ask(c, 1, &[]);
    broker.register(q.clone()).unwrap();

    let start = broker.listing_start();
    broker.mark_attached(&q.id);
    broker.reconcile_panes(start, &[pane("w1:p1", AgentStatus::Idle)]);
    poll(&broker, AgentStatus::Working);
    assert_eq!(broker.snapshot(), vec![q]);
}

#[test]
fn reports_changes() {
    let broker = Broker::new();
    let changes = broker.changes();
    let c = client();
    let (_rx, guard) = broker.connect(c);
    let q = ask(c, 1, &[]);
    broker.register(q.clone()).unwrap();
    let answered = ask(c, 2, &["Yes"]);
    broker.register(answered.clone()).unwrap();
    broker.answer(&answered.id, "Yes".into()).unwrap();
    broker.withdraw(&q.id, WithdrawReason::Deleted);
    let before = *changes.borrow();
    broker.withdraw(&q.id, WithdrawReason::Deleted);
    assert_eq!(
        broker.answer(&q.id, "Yes".into()),
        Err(AnswerError::NotFound)
    );
    assert_eq!(*changes.borrow(), before, "no-ops must not notify");
    drop(guard);
    // connect・登録 2 回・回答・取り下げ・切断
    assert_eq!(*changes.borrow(), 6);
}
