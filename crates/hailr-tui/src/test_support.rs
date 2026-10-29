use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hailr_core::mux::{Mux, MuxError, PaneInfo};
use hailr_core::protocol::{ClientId, PaneId, Question, QuestionBody, QuestionId};
use tokio::sync::mpsc;
use uuid::Uuid;

/// 既定値の ask。id と client は毎回新しく採番する
pub fn ask() -> Question {
    Question {
        id: QuestionId(Uuid::new_v4()),
        client_id: ClientId(Uuid::new_v4()),
        pane_id: PaneId("w1:p1".into()),
        asked_at: 1,
        body: QuestionBody::Ask {
            title: "t".into(),
            question: "q".into(),
            options: vec![],
            links: vec![],
        },
    }
}

/// 既定値の permission
pub fn permission() -> Question {
    Question {
        body: QuestionBody::Permission {
            request_id: "abcde".into(),
            tool_name: "Bash".into(),
            description: "d".into(),
            input_preview: "p".into(),
        },
        ..ask()
    }
}

/// 既定値から一部だけ変えるための設定。種類の違う body に指定したら panic する
pub trait QuestionExt: Sized {
    fn client(self, client: ClientId) -> Self;
    fn pane(self, pane: &str) -> Self;
    fn asked_at(self, asked_at: u64) -> Self;
    fn title(self, title: &str) -> Self;
    fn text(self, text: &str) -> Self;
    fn options(self, options: &[&str]) -> Self;
    fn description(self, description: &str) -> Self;
    fn input_preview(self, input_preview: &str) -> Self;
}

impl QuestionExt for Question {
    fn client(mut self, client: ClientId) -> Self {
        self.client_id = client;
        self
    }

    fn pane(mut self, pane: &str) -> Self {
        self.pane_id = PaneId(pane.into());
        self
    }

    fn asked_at(mut self, asked_at: u64) -> Self {
        self.asked_at = asked_at;
        self
    }

    fn title(mut self, value: &str) -> Self {
        if let QuestionBody::Ask { title, .. } = &mut self.body {
            *title = value.into();
        } else {
            panic!("title is for an Ask question");
        }
        self
    }

    fn text(mut self, value: &str) -> Self {
        if let QuestionBody::Ask { question, .. } = &mut self.body {
            *question = value.into();
        } else {
            panic!("text is for an Ask question");
        }
        self
    }

    fn options(mut self, values: &[&str]) -> Self {
        if let QuestionBody::Ask { options, .. } = &mut self.body {
            *options = values.iter().map(|o| o.to_string()).collect();
        } else {
            panic!("options is for an Ask question");
        }
        self
    }

    fn description(mut self, value: &str) -> Self {
        if let QuestionBody::Permission { description, .. } = &mut self.body {
            *description = value.into();
        } else {
            panic!("description is for a Permission question");
        }
        self
    }

    fn input_preview(mut self, value: &str) -> Self {
        if let QuestionBody::Permission { input_preview, .. } = &mut self.body {
            *input_preview = value.into();
        } else {
            panic!("input_preview is for a Permission question");
        }
        self
    }
}

/// 偽の Mux。読んだ pane を覚えて "bad" だけ読みに失敗し、
/// `notifying` で作ると notify の値を送り出して失敗を返し、同時に走った数の最大も数える
#[derive(Default)]
pub struct FakeMux {
    pub read: Mutex<Vec<String>>,
    sent: Option<mpsc::UnboundedSender<(String, String)>>,
    running: AtomicUsize,
    pub most: AtomicUsize,
}

impl FakeMux {
    pub fn notifying(sent: mpsc::UnboundedSender<(String, String)>) -> Self {
        Self {
            sent: Some(sent),
            ..Self::default()
        }
    }
}

impl Mux for FakeMux {
    fn list_panes(&self) -> Result<Vec<PaneInfo>, MuxError> {
        unimplemented!()
    }

    fn read_pane(&self, pane: &PaneId) -> Result<String, MuxError> {
        self.read.lock().unwrap().push(pane.0.clone());
        if pane.0 == "bad" {
            return Err(MuxError::InvalidPane);
        }
        Ok(format!("screen of {}", pane.0))
    }

    fn focus(&self, _: &PaneId) -> Result<(), MuxError> {
        unimplemented!()
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), MuxError> {
        let Some(sent) = &self.sent else {
            unimplemented!("notify on a FakeMux made without notifying");
        };
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(5));
        self.running.fetch_sub(1, Ordering::SeqCst);
        sent.send((title.into(), body.into())).unwrap();
        Err(MuxError::InvalidPane)
    }
}
