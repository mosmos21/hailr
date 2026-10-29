use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use hailr_core::protocol::{ClientId, Question};
use tokio_stream::Stream;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use super::{Broker, RegisterError, Registered};

const KEEP_ALIVE: Duration = Duration::from_secs(15);

pub fn router(broker: Arc<Broker>) -> Router {
    Router::new()
        .route("/questions", post(register))
        .route("/clients/{client_id}/events", get(events))
        .with_state(broker)
}

async fn register(
    State(broker): State<Arc<Broker>>,
    Json(question): Json<Question>,
) -> (StatusCode, String) {
    match broker.register(question) {
        Ok(Registered::New) => (StatusCode::CREATED, String::new()),
        Ok(Registered::Updated) => (StatusCode::OK, String::new()),
        Err(e @ (RegisterError::NoClient | RegisterError::NotOwner)) => {
            (StatusCode::CONFLICT, e.to_string())
        }
        Err(e @ RegisterError::Invalid(_)) => (StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn events(
    State(broker): State<Arc<Broker>>,
    Path(client): Path<ClientId>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (rx, guard) = broker.connect(client);
    // guard をストリームに持たせ、接続が切れてストリームが drop されたときに client を外す
    let stream = ReceiverStream::new(rx).map(move |event| {
        let _connected = &guard;
        Ok(Event::default()
            .json_data(event)
            .expect("ClientEvent always serializes"))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, Response};
    use hailr_core::protocol::{ClientEvent, PaneId};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::*;
    use crate::test_support::{self, QuestionExt};

    fn question(client: ClientId, options: &[&str]) -> Question {
        test_support::ask().client(client).options(options)
    }

    async fn post(broker: &Arc<Broker>, body: String) -> StatusCode {
        let request = Request::post("/questions")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        router(Arc::clone(broker))
            .oneshot(request)
            .await
            .unwrap()
            .status()
    }

    async fn subscribe(broker: &Arc<Broker>, client: ClientId) -> Response<Body> {
        let request = Request::get(format!("/clients/{}/events", client.0))
            .body(Body::empty())
            .unwrap();
        router(Arc::clone(broker)).oneshot(request).await.unwrap()
    }

    async fn next_event(response: &mut Response<Body>) -> ClientEvent {
        let frame = response.body_mut().frame().await.unwrap().unwrap();
        let text = String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap();
        let data = text.strip_prefix("data: ").unwrap().trim_end();
        serde_json::from_str(data).unwrap()
    }

    #[tokio::test]
    async fn registers_only_while_the_client_is_subscribed() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let q = question(client, &[]);
        let body = serde_json::to_string(&q).unwrap();

        assert_eq!(post(&broker, body.clone()).await, StatusCode::CONFLICT);
        let events = subscribe(&broker, client).await;
        assert_eq!(events.headers()["content-type"], "text/event-stream");
        assert_eq!(post(&broker, body.clone()).await, StatusCode::CREATED);
        assert_eq!(post(&broker, body.clone()).await, StatusCode::OK);

        drop(events);
        assert_eq!(post(&broker, body).await, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn rejects_invalid_questions() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let _events = subscribe(&broker, client).await;
        let mut q = question(client, &[]);
        q.pane_id = PaneId("-x".into());
        let body = serde_json::to_string(&q).unwrap();
        assert_eq!(post(&broker, body).await, StatusCode::BAD_REQUEST);
        assert_eq!(
            post(&broker, "{}".into()).await,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn streams_answers_to_the_asking_client() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let mut events = subscribe(&broker, client).await;
        let q = question(client, &["Yes", "No"]);
        post(&broker, serde_json::to_string(&q).unwrap()).await;

        broker.answer(&q.id, "Yes".into()).unwrap();
        assert_eq!(
            next_event(&mut events).await,
            ClientEvent::Answered {
                id: q.id,
                answer: "Yes".into()
            }
        );
    }

    #[tokio::test]
    async fn rejects_malformed_client_ids() {
        let request = Request::get("/clients/not-a-uuid/events")
            .body(Body::empty())
            .unwrap();
        let response = router(Broker::new()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
