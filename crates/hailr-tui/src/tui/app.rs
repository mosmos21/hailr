use hailr_core::protocol::{PaneId, Question, QuestionId};
use ratatui::text::Text;

/// pane の画面を読んだ結果。読めなかったときは理由を持つ
pub type Shot = (PaneId, Result<Text<'static>, String>);

/// 画面に出すものと、選んでいる質問
#[derive(Default)]
pub struct App {
    questions: Vec<Question>,
    selected: Option<QuestionId>,
    shot: Option<Shot>,
}

impl App {
    /// 選んでいた質問が消えたら、同じ位置（無ければ最後）の質問に移る
    pub fn set_questions(&mut self, questions: Vec<Question>) {
        let position = self.selected_index().unwrap_or(0);
        self.questions = questions;
        if self.selected_index().is_none() {
            let last = self.questions.len().checked_sub(1);
            self.selected = last.map(|last| self.questions[position.min(last)].id);
        }
    }

    pub fn select_next(&mut self) {
        self.move_selection(|i| i + 1);
    }

    pub fn select_previous(&mut self) {
        self.move_selection(|i| i.saturating_sub(1));
    }

    /// 選んでいる pane の結果だけを受け取る。前に選んでいた pane の結果が遅れて届いたら捨てる
    pub fn set_shot(&mut self, shot: Shot) {
        if self.selected_pane().as_ref() == Some(&shot.0) {
            self.shot = Some(shot);
        }
    }

    /// 選ぶ pane が変わったら呼ぶ。前に読んだ同じ pane の古い画面を、読み直すまで出さない
    pub fn forget_shot(&mut self) {
        self.shot = None;
    }

    pub fn questions(&self) -> &[Question] {
        &self.questions
    }

    pub fn selected_index(&self) -> Option<usize> {
        let id = self.selected?;
        self.questions.iter().position(|q| q.id == id)
    }

    pub fn selected(&self) -> Option<&Question> {
        self.selected_index().map(|i| &self.questions[i])
    }

    pub fn selected_pane(&self) -> Option<PaneId> {
        self.selected().map(|q| q.pane_id.clone())
    }

    /// 選んでいる pane の結果。選び直したあと、まだ読めていなければ None
    pub fn shot(&self) -> Option<&Result<Text<'static>, String>> {
        let (pane, result) = self.shot.as_ref()?;
        (Some(pane) == self.selected().map(|q| &q.pane_id)).then_some(result)
    }

    fn move_selection(&mut self, step: impl Fn(usize) -> usize) {
        let Some(current) = self.selected_index() else {
            return;
        };
        let next = step(current).min(self.questions.len() - 1);
        self.selected = Some(self.questions[next].id);
    }
}

#[cfg(test)]
mod tests {
    use hailr_core::protocol::{ClientId, QuestionBody};
    use uuid::Uuid;

    use super::*;

    fn question(pane: &str) -> Question {
        Question {
            id: QuestionId(Uuid::new_v4()),
            client_id: ClientId(Uuid::new_v4()),
            pane_id: PaneId(pane.into()),
            asked_at: 1,
            body: QuestionBody::Ask {
                title: "t".into(),
                question: "q".into(),
                options: vec![],
                links: vec![],
            },
        }
    }

    #[test]
    fn selects_the_first_question_and_stops_at_both_ends() {
        let mut app = App::default();
        assert_eq!(app.selected_index(), None);
        app.select_next();
        assert_eq!(app.selected_index(), None);

        let qs = vec![question("a"), question("b")];
        app.set_questions(qs.clone());
        assert_eq!(app.selected_index(), Some(0));
        app.select_previous();
        assert_eq!(app.selected_index(), Some(0));
        app.select_next();
        app.select_next();
        assert_eq!(app.selected_index(), Some(1));
    }

    #[test]
    fn keeps_the_selection_or_takes_the_same_position() {
        let mut app = App::default();
        let [a, b, c] = [question("a"), question("b"), question("c")];
        app.set_questions(vec![a.clone(), b.clone(), c.clone()]);
        app.select_next();

        app.set_questions(vec![a.clone(), b.clone()]);
        assert_eq!(app.selected().unwrap().id, b.id, "kept");
        app.set_questions(vec![a.clone(), c.clone()]);
        assert_eq!(app.selected().unwrap().id, c.id, "same position");
        app.set_questions(vec![a.clone()]);
        assert_eq!(app.selected().unwrap().id, a.id, "last");
        app.set_questions(vec![]);
        assert!(app.selected().is_none());

        let d = question("d");
        app.set_questions(vec![a.clone(), b.clone(), c.clone()]);
        app.select_next();
        app.set_questions(vec![a, c.clone(), d]);
        assert_eq!(
            app.selected().unwrap().id,
            c.id,
            "same position, not the last"
        );
    }

    #[test]
    fn shows_only_the_shot_of_the_selected_pane() {
        let mut app = App::default();
        app.set_questions(vec![question("a"), question("b")]);
        app.set_shot((PaneId("b".into()), Err("late".into())));
        assert!(app.shot().is_none(), "a result for another pane is dropped");

        app.set_shot((PaneId("a".into()), Err("failed".into())));
        assert_eq!(app.shot(), Some(&Err("failed".into())));
        app.select_next();
        assert!(app.shot().is_none(), "the old pane's result is hidden");
    }
}
