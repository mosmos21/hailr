use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use hailr_core::protocol::{
    ClientEvent, ClientId, MAX_LINKS, MAX_OPTIONS, Question, QuestionBody, QuestionId,
    WithdrawReason,
};
use tokio::sync::{mpsc, watch};

pub mod http;
pub mod socket;

const CLIENT_QUEUE: usize = 16;

/// 全 pane の未回答の質問の写しと、接続中の ask-mcp を持つ。質問の正は ask-mcp 側にある
pub struct Broker {
    state: Mutex<State>,
    changes: watch::Sender<u64>,
}

#[derive(Default)]
struct State {
    questions: HashMap<QuestionId, Entry>,
    clients: HashMap<ClientId, Client>,
    next_connection: u64,
}

struct Entry {
    question: Question,
}

struct Client {
    connection: u64,
    events: mpsc::Sender<ClientEvent>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Registered {
    New,
    Updated,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    #[error("the client is not connected")]
    NoClient,
    #[error("the question id belongs to another client")]
    NotOwner,
    #[error("invalid question: {0}")]
    Invalid(&'static str),
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AnswerError {
    #[error("no such question")]
    NotFound,
    #[error("the answer is not one of the options")]
    NotAnOption,
    #[error("the asking client could not take the answer; it will reconnect and ask again")]
    Undelivered,
}

/// SSE のストリームに持たせ、接続が切れて drop されたらその client と質問を消す
pub struct ClientGuard {
    broker: Arc<Broker>,
    client: ClientId,
    connection: u64,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.broker.disconnect(self.client, self.connection);
    }
}

impl Broker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::default(),
            changes: watch::Sender::new(0),
        })
    }

    pub fn connect(
        self: &Arc<Self>,
        client: ClientId,
    ) -> (mpsc::Receiver<ClientEvent>, ClientGuard) {
        let (tx, rx) = mpsc::channel(CLIENT_QUEUE);
        let mut state = self.lock();
        state.next_connection += 1;
        let connection = state.next_connection;
        // つなぎ直した ask-mcp は手元の未回答を全部登録し直すため、前の接続の写しは捨てる
        state
            .questions
            .retain(|_, entry| entry.question.client_id != client);
        // 同じ client が切断に気づく前につなぎ直すことがあるため、古い接続の後始末が新しい接続を消さないよう世代で区別する
        state.clients.insert(
            client,
            Client {
                connection,
                events: tx,
            },
        );
        drop(state);
        self.changed();
        let guard = ClientGuard {
            broker: Arc::clone(self),
            client,
            connection,
        };
        (rx, guard)
    }

    pub fn register(&self, question: Question) -> Result<Registered, RegisterError> {
        validate(&question)?;
        let mut state = self.lock();
        if !state.clients.contains_key(&question.client_id) {
            return Err(RegisterError::NoClient);
        }
        let registered = match state.questions.get_mut(&question.id) {
            Some(entry) if entry.question.client_id != question.client_id => {
                return Err(RegisterError::NotOwner);
            }
            Some(entry) => {
                entry.question = question;
                Registered::Updated
            }
            None => {
                state.questions.insert(question.id, Entry { question });
                Registered::New
            }
        };
        drop(state);
        self.changed();
        Ok(registered)
    }

    /// 未回答の質問を古い順に返す
    pub fn snapshot(&self) -> Vec<Question> {
        let mut questions: Vec<_> = self
            .lock()
            .questions
            .values()
            .map(|entry| entry.question.clone())
            .collect();
        questions.sort_by_key(|q| (q.asked_at, q.id));
        questions
    }

    pub fn answer(&self, id: &QuestionId, answer: String) -> Result<(), AnswerError> {
        let mut state = self.lock();
        let question = &state
            .questions
            .get(id)
            .ok_or(AnswerError::NotFound)?
            .question;
        if !question.body.options().contains(&answer.as_str()) {
            return Err(AnswerError::NotAnOption);
        }
        let client = question.client_id;
        let delivered = send(
            &mut state,
            client,
            ClientEvent::Answered { id: *id, answer },
        );
        state.questions.remove(id);
        drop(state);
        self.changed();
        if delivered {
            Ok(())
        } else {
            Err(AnswerError::Undelivered)
        }
    }

    pub fn withdraw(&self, id: &QuestionId, reason: WithdrawReason) {
        let mut state = self.lock();
        let Some(entry) = state.questions.remove(id) else {
            return;
        };
        send(
            &mut state,
            entry.question.client_id,
            ClientEvent::Withdrawn { id: *id, reason },
        );
        drop(state);
        self.changed();
    }

    /// 状態が変わるたびに値が増える
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn disconnect(&self, client: ClientId, connection: u64) {
        let mut state = self.lock();
        if state.clients.get(&client).map(|c| c.connection) != Some(connection) {
            return;
        }
        state.clients.remove(&client);
        state
            .questions
            .retain(|_, entry| entry.question.client_id != client);
        drop(state);
        self.changed();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn changed(&self) {
        self.changes.send_modify(|n| *n += 1);
    }
}

/// 送れなかった client は切断として扱う。送り手を手放すと SSE が閉じ、ask-mcp がつなぎ直して再登録する
fn send(state: &mut State, client: ClientId, event: ClientEvent) -> bool {
    let Some(target) = state.clients.get(&client) else {
        return false;
    };
    if target.events.try_send(event).is_ok() {
        return true;
    }
    state.clients.remove(&client);
    state
        .questions
        .retain(|_, entry| entry.question.client_id != client);
    false
}

fn validate(question: &Question) -> Result<(), RegisterError> {
    let pane = &question.pane_id.0;
    // herdr の CLI は `-` で始まる引数をオプションとして読むため断る
    if pane.is_empty() || pane.starts_with('-') {
        return Err(RegisterError::Invalid("paneId"));
    }
    if let QuestionBody::Ask { options, links, .. } = &question.body {
        if options.len() > MAX_OPTIONS {
            return Err(RegisterError::Invalid("too many options"));
        }
        // ボタンの文言で回答を返すため、空や重複があると押したボタンを区別できない
        let mut seen = std::collections::HashSet::new();
        if !options
            .iter()
            .all(|o| !o.trim().is_empty() && seen.insert(o))
        {
            return Err(RegisterError::Invalid(
                "options must be non-empty and distinct",
            ));
        }
        if links.len() > MAX_LINKS {
            return Err(RegisterError::Invalid("too many links"));
        }
        if !links.iter().all(|l| is_web_url(l)) {
            return Err(RegisterError::Invalid("links must be http(s) URLs"));
        }
    }
    Ok(())
}

// ブラウザで開く値なので、ホストのある http(s) の URL だけを通す
fn is_web_url(link: &str) -> bool {
    if link.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    url::Url::parse(link).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|h| !h.is_empty())
    })
}

#[cfg(test)]
mod tests;
