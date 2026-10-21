use hailr_core::protocol::{
    ClientEvent, PERMISSION_ALLOW, PERMISSION_DENY, Question, QuestionBody, WithdrawReason,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

fn round_trip<T: Serialize + DeserializeOwned>(json: &str) -> T {
    let expected: serde_json::Value = serde_json::from_str(json).unwrap();
    let parsed: T = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_value(&parsed).unwrap(), expected);
    parsed
}

#[test]
fn ask_question() {
    let q: Question = round_trip(include_str!("../../../fixtures/protocol/question-ask.json"));
    assert_eq!(q.body.title(), "Merge PR #123?");
    assert_eq!(q.body.options(), ["Yes, merge", "No", "Wait for review"]);
    assert_eq!(q.body.links().len(), 1);
}

#[test]
fn ask_question_without_options() {
    let q: Question = round_trip(include_str!(
        "../../../fixtures/protocol/question-ask-minimal.json"
    ));
    assert!(q.body.options().is_empty());
}

#[test]
fn ask_question_defaults_missing_lists() {
    let body: QuestionBody =
        serde_json::from_str(r#"{"kind":"ask","title":"t","question":"q"}"#).unwrap();
    assert!(body.options().is_empty());
    assert!(body.links().is_empty());
}

#[test]
fn permission_question() {
    let q: Question = round_trip(include_str!(
        "../../../fixtures/protocol/question-permission.json"
    ));
    assert_eq!(q.body.title(), "Allow Bash?");
    assert_eq!(q.body.options(), [PERMISSION_ALLOW, PERMISSION_DENY]);
    assert!(q.body.links().is_empty());
}

#[test]
fn answered_event() {
    let e: ClientEvent = round_trip(include_str!(
        "../../../fixtures/protocol/event-answered.json"
    ));
    assert!(matches!(e, ClientEvent::Answered { answer, .. } if answer == "Yes, merge"));
}

#[test]
fn withdrawn_events() {
    let cases = [
        (
            include_str!("../../../fixtures/protocol/event-withdrawn.json"),
            WithdrawReason::AnsweredInPane,
        ),
        (
            include_str!("../../../fixtures/protocol/event-withdrawn-deleted.json"),
            WithdrawReason::Deleted,
        ),
        (
            include_str!("../../../fixtures/protocol/event-withdrawn-pane-closed.json"),
            WithdrawReason::PaneClosed,
        ),
    ];
    for (json, expected) in cases {
        let e: ClientEvent = round_trip(json);
        assert!(matches!(e, ClientEvent::Withdrawn { reason, .. } if reason == expected));
    }
}
