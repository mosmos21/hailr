use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use hailr_core::mux::{AgentStatus, PaneInfo};
use hailr_core::protocol::{
    ClientEvent, ClientId, MAX_LINKS, MAX_OPTIONS, Question, QuestionBody, QuestionId,
    WithdrawReason,
};
use tokio::sync::{broadcast, mpsc, watch};

pub mod http;
pub mod notifier;
pub mod poller;
pub mod serve;
pub mod socket;

const CLIENT_QUEUE: usize = 16;
const ARRIVALS: usize = 16;
// tokio の broadcast は容量を 2 のべき乗に切り上げるため、溢れる件数を ARRIVALS どおりにする
const _: () = assert!(ARRIVALS.is_power_of_two());

/// 全 pane の未回答の質問の写しと、接続中の ask-mcp を持つ。質問の正は ask-mcp 側にある
pub struct Broker {
    state: Mutex<State>,
    changes: watch::Sender<u64>,
    arrivals: broadcast::Sender<QuestionId>,
}

#[derive(Default)]
struct State {
    questions: HashMap<QuestionId, Entry>,
    clients: HashMap<ClientId, Client>,
    next_connection: u64,
    attaches: u64,
    /// つなぎ直した ask-mcp の再登録で鳴らし直さないよう、回答・取り下げまで覚えておく
    notified: HashSet<QuestionId>,
}

struct Entry {
    question: Question,
    attach: Attach,
}

/// TUI から pane に attach したあと、pane が待ち（idle / blocked）を経て working になったら、人が pane で答えたとみなす
#[derive(Clone, Copy, PartialEq, Eq)]
enum Attach {
    No,
    /// attach したが、まだ待ちを見ていない。attach の前から別の作業で working のことがあるため。値は attach の通し番号
    Marked(u64),
    /// attach したあとに待ちを見た。次に working になったら答えたとみなす
    Armed,
}

/// 一覧を取り始めた時点を表す。これより後の attach は、この一覧では進めない
#[derive(Clone, Copy, Debug)]
pub struct ListingStart(u64);

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
            arrivals: broadcast::Sender::new(ARRIVALS),
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
                if state.notified.insert(question.id) {
                    // 受け手が居ないときは知らせる相手が居ないだけなので、送れなくても捨てる
                    let _ = self.arrivals.send(question.id);
                }
                state.questions.insert(
                    question.id,
                    Entry {
                        question,
                        attach: Attach::No,
                    },
                );
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
        state.notified.remove(id);
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
        let withdrawn = withdraw(&mut state, id, reason);
        drop(state);
        if withdrawn {
            self.changed();
        }
    }

    /// つなぎ直しで写しが作り直されると印も消えるので、TUI は attach から戻ったときにも呼び直す。
    /// attach の間に待ちを見ていたら、戻ってすぐ答えが始まっても数えられるよう、そのままにする
    pub fn mark_attached(&self, id: &QuestionId) {
        let mut state = self.lock();
        state.attaches += 1;
        let seq = state.attaches;
        if let Some(entry) = state.questions.get_mut(id)
            && entry.attach != Attach::Armed
        {
            entry.attach = Attach::Marked(seq);
        }
    }

    /// 一覧を取り始める直前に呼び、その一覧を渡す `reconcile_panes` に渡す
    pub fn listing_start(&self) -> ListingStart {
        ListingStart(self.lock().attaches)
    }

    /// attach して pane で答えた質問を取り下げる。pane が一覧に無いだけでは取り下げない。herdr の一覧は
    /// agent の居る pane しか返さず、pane が閉じたときは ask-mcp の接続が切れてその質問が消えるため
    pub fn reconcile_panes(&self, start: ListingStart, panes: &[PaneInfo]) {
        let mut state = self.lock();
        let mut answered = Vec::new();
        for entry in state.questions.values_mut() {
            let Some(pane) = panes.iter().find(|p| p.id == entry.question.pane_id) else {
                continue;
            };
            let waiting = matches!(pane.status, AgentStatus::Idle | AgentStatus::Blocked);
            match entry.attach {
                // attach より前に取り始めた一覧は attach 前の様子なので、待ちに見えても進めない
                Attach::Marked(seq) if waiting && seq <= start.0 => entry.attach = Attach::Armed,
                Attach::Armed if pane.status == AgentStatus::Working => {
                    answered.push(entry.question.id)
                }
                _ => {}
            }
        }
        // 判定から取り下げまでの間に同じ質問が回答・再登録されないよう、同じロックの中で取り下げる
        for id in &answered {
            withdraw(&mut state, id, WithdrawReason::AnsweredInPane);
        }
        drop(state);
        if !answered.is_empty() {
            self.changed();
        }
    }

    /// 全 client の送り手を手放す。SSE のストリームが終わり、ask-mcp の接続が閉じる
    pub fn close_all(&self) {
        let mut state = self.lock();
        state.clients.clear();
        state.questions.clear();
        state.notified.clear();
        drop(state);
        self.changed();
    }

    /// 通知する質問の今の写しを返す。無ければ通知済みから外し、切断中だった質問がつなぎ直して再登録されたときに鳴らす
    pub fn to_announce(&self, id: &QuestionId) -> Option<Question> {
        let mut state = self.lock();
        let question = state.questions.get(id).map(|entry| entry.question.clone());
        if question.is_none() {
            state.notified.remove(id);
        }
        question
    }

    /// 初めて届いた質問の ID を受け取る
    pub fn arrivals(&self) -> broadcast::Receiver<QuestionId> {
        self.arrivals.subscribe()
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

/// 質問を消して持ち主に知らせる。消したら true
fn withdraw(state: &mut State, id: &QuestionId, reason: WithdrawReason) -> bool {
    let Some(entry) = state.questions.remove(id) else {
        return false;
    };
    state.notified.remove(id);
    let event = ClientEvent::Withdrawn { id: *id, reason };
    send(state, entry.question.client_id, event);
    true
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
