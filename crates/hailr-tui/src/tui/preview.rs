use std::sync::Arc;
use std::time::Duration;

use ansi_to_tui::IntoText;
use hailr_core::mux::Mux;
use hailr_core::protocol::PaneId;
use ratatui::style::Modifier;
use ratatui::text::Text;
use tokio::sync::{mpsc, watch};

use super::app::Shot;
use crate::text::printable;

pub const INTERVAL: Duration = Duration::from_secs(1);

/// 選んでいる pane の画面を 1 秒ごとと選び直したときに読む。read_pane は最長 2 秒待つので、描画のループの外で回す
pub async fn run(
    mux: Arc<dyn Mux>,
    mut pane: watch::Receiver<Option<PaneId>>,
    shots: mpsc::Sender<Shot>,
) {
    let mut ticks = tokio::time::interval(INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            changed = pane.changed() => if changed.is_err() { return },
        }
        let Some(id) = pane.borrow_and_update().clone() else {
            continue;
        };
        let reader = Arc::clone(&mux);
        let target = id.clone();
        let result = match tokio::task::spawn_blocking(move || reader.read_pane(&target)).await {
            Ok(Ok(ansi)) => Ok(to_text(&ansi)),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(format!("reading the pane panicked: {e}")),
        };
        if shots.send((id, result)).await.is_err() {
            return;
        }
        // 選び直して読んだ直後に、溜まっていた tick で同じ pane をもう一度読まないよう数え直す
        ticks.reset();
    }
}

/// 色は残し、色以外のエスケープや制御文字は描く前に空白にする
fn to_text(ansi: &str) -> Text<'static> {
    let mut text = ansi
        .as_bytes()
        .into_text()
        .unwrap_or_else(|_| Text::raw(ansi.to_owned()));
    for line in &mut text.lines {
        for span in &mut line.spans {
            if span.content.chars().any(crate::text::is_unprintable) {
                span.content = printable(&span.content).into();
            }
            span.style
                .add_modifier
                .remove(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use hailr_core::mux::{MuxError, PaneInfo};
    use ratatui::style::{Color, Style};

    use super::*;

    #[test]
    fn keeps_colors_and_blanks_other_escapes() {
        let text = to_text("\u{1b}[5;31mred\u{1b}[0m \u{1b}]52;c;aGk=\u{7}x\u{202e}y\nnext");
        let line: String = text.lines[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!line.chars().any(crate::text::is_unprintable), "{line:?}");
        assert!(line.starts_with("red"), "{line:?}");
        assert_eq!(text.lines[0].spans[0].style, Style::new().fg(Color::Red));
        assert_eq!(text.lines.len(), 2);
    }

    /// 読んだ pane を返し、"bad" だけ失敗する偽の Mux
    struct FakeMux(Mutex<Vec<String>>);

    impl Mux for FakeMux {
        fn list_panes(&self) -> Result<Vec<PaneInfo>, MuxError> {
            unimplemented!()
        }
        fn read_pane(&self, pane: &PaneId) -> Result<String, MuxError> {
            self.0.lock().unwrap().push(pane.0.clone());
            if pane.0 == "bad" {
                return Err(MuxError::InvalidPane);
            }
            Ok(format!("screen of {}", pane.0))
        }
        fn focus(&self, _: &PaneId) -> Result<(), MuxError> {
            unimplemented!()
        }
        fn notify(&self, _: &str, _: &str) -> Result<(), MuxError> {
            unimplemented!()
        }
    }

    /// 選び直してから tick より早く届くことを、tick の間隔より短い期限で確かめる
    async fn next(shots: &mut mpsc::Receiver<Shot>) -> Shot {
        tokio::time::timeout(INTERVAL / 2, shots.recv())
            .await
            .expect("read before the next tick")
            .unwrap()
    }

    #[tokio::test]
    async fn reads_the_selected_pane_right_away_and_reports_failures() {
        let (pane_tx, pane_rx) = watch::channel(None);
        let (shot_tx, mut shots) = mpsc::channel(4);
        let mux = Arc::new(FakeMux(Mutex::default()));
        let reading = tokio::spawn(run(mux.clone(), pane_rx, shot_tx));
        tokio::task::yield_now().await;

        pane_tx.send(Some(PaneId("a".into()))).unwrap();
        let (pane, result) = next(&mut shots).await;
        assert_eq!(pane, PaneId("a".into()));
        assert_eq!(result.unwrap(), Text::raw("screen of a"));
        pane_tx.send(Some(PaneId("bad".into()))).unwrap();
        let (pane, result) = next(&mut shots).await;
        assert_eq!(pane, PaneId("bad".into()));
        assert!(result.is_err());
        assert_eq!(
            *mux.0.lock().unwrap(),
            ["a", "bad"],
            "nothing read while unselected"
        );

        drop(pane_tx);
        reading.await.unwrap();
    }
}
