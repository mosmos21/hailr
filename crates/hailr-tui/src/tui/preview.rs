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
    use ratatui::style::{Color, Style};

    use super::*;
    use crate::test_support::FakeMux;

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

    /// 選び直してから tick より早く届くことを、tick の間隔より短い期限で確かめる
    async fn next(shots: &mut mpsc::Receiver<Shot>) -> Shot {
        tokio::time::timeout(INTERVAL / 2, shots.recv())
            .await
            .expect("read before the next tick")
            .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn reads_the_selected_pane_right_away_and_reports_failures() {
        let (pane_tx, pane_rx) = watch::channel(None);
        let (shot_tx, mut shots) = mpsc::channel(4);
        let mux = Arc::new(FakeMux::default());
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
            *mux.read.lock().unwrap(),
            ["a", "bad"],
            "nothing read while unselected"
        );

        drop(pane_tx);
        reading.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn rereads_the_selected_pane_every_interval() {
        let (pane_tx, pane_rx) = watch::channel(None);
        let (shot_tx, mut shots) = mpsc::channel(4);
        let mux = Arc::new(FakeMux::default());
        let reading = tokio::spawn(run(mux.clone(), pane_rx, shot_tx));
        tokio::task::yield_now().await;

        pane_tx.send(Some(PaneId("a".into()))).unwrap();
        assert!(next(&mut shots).await.1.is_ok());
        for _ in 0..2 {
            tokio::time::advance(INTERVAL - Duration::from_millis(2)).await;
            // 読みが走っている間は仮想時間が進まないため、窓の中に読みがあれば期限より先に届く
            let early = tokio::time::timeout(Duration::from_millis(1), shots.recv()).await;
            assert!(early.is_err(), "nothing read before the interval");
            tokio::time::advance(Duration::from_millis(2)).await;
            let (pane, result) = tokio::time::timeout(Duration::from_millis(1), shots.recv())
                .await
                .expect("read right at the interval")
                .unwrap();
            assert_eq!(pane, PaneId("a".into()));
            assert!(result.is_ok());
        }
        assert_eq!(*mux.read.lock().unwrap(), ["a", "a", "a"]);

        drop(pane_tx);
        reading.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn reports_a_panicking_read_and_keeps_reading() {
        let (pane_tx, pane_rx) = watch::channel(None);
        let (shot_tx, mut shots) = mpsc::channel(4);
        let mux = Arc::new(FakeMux::default());
        let reading = tokio::spawn(run(mux.clone(), pane_rx, shot_tx));
        tokio::task::yield_now().await;

        pane_tx.send(Some(PaneId("panic".into()))).unwrap();
        let (pane, result) = next(&mut shots).await;
        assert_eq!(pane, PaneId("panic".into()));
        assert!(result.unwrap_err().starts_with("reading the pane panicked"));
        tokio::time::advance(INTERVAL).await;
        let (_, result) = next(&mut shots).await;
        assert!(result.unwrap_err().starts_with("reading the pane panicked"));
        pane_tx.send(Some(PaneId("a".into()))).unwrap();
        let (_, result) = next(&mut shots).await;
        assert_eq!(result.unwrap(), Text::raw("screen of a"));

        drop(pane_tx);
        reading.await.unwrap();
    }
}
