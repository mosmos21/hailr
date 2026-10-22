use std::cell::{Cell, RefCell};
use std::time::Instant;

use hailr_core::protocol::{PaneId, Question, QuestionBody, QuestionId};
use ratatui::layout::{Position, Rect};
use ratatui::text::Text;

use crate::broker::MAX_REPLY;

/// pane の画面を読んだ結果。読めなかったときは理由を持つ
pub type Shot = (PaneId, Result<Text<'static>, String>);

/// クリックで選べるもの
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Question(QuestionId),
    /// 0 から数えたボタンの位置
    Button(usize),
    /// 0 から数えたリンクの位置
    Link(usize),
}

/// 画面に出すものと、選んでいる質問
#[derive(Default)]
pub struct App {
    questions: Vec<Question>,
    selected: Option<QuestionId>,
    shot: Option<Shot>,
    notice: Option<String>,
    /// 選んでいた質問が消えてから、人がまだ選び直していない。一覧が空を挟んでも消さない
    moved: bool,
    /// 直前の描画で見えていたボタンの数。見えていない番号では答えさせない
    shown_buttons: Cell<usize>,
    /// `o` を押し、開くリンクの番号を待っている。数字と `o` 以外のキーで消す
    opening: bool,
    /// 待ちの中で直前に開いたリンク。押しっぱなしで同じリンクを何度も開かない
    opened: Option<usize>,
    shown_links: Cell<usize>,
    /// 直前の描画でクリックできた場所
    targets: RefCell<Vec<(Rect, Target)>>,
    answered: Option<Answered>,
    answered_for: Option<std::time::Duration>,
    replying: Option<Replying>,
}

struct Replying {
    id: QuestionId,
    text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// 0 から数えたボタンの位置
    Button(usize),
    Reply(String),
}

/// 押したボタンが効いたと分かるよう、答えた質問を期限まで一覧に残す
struct Answered {
    question: Question,
    choice: Choice,
    until: Instant,
}

impl App {
    /// 選んでいた質問が消えたら、同じ位置（無ければ最後）の質問に移る
    pub fn set_questions(&mut self, mut questions: Vec<Question>) {
        let position = self.selected_index().unwrap_or(0);
        // 同じ質問が聞き直されたら、もう答えたものとして見せない
        if let Some(answered) = &self.answered
            && questions.iter().any(|q| q.id == answered.question.id)
        {
            // 押した数字のキーリピートが聞き直された質問に届かないよう、選び直すまで答えさせない
            if self.selected == Some(answered.question.id) {
                self.moved = true;
            }
            self.answered = None;
        }
        if let Some(answered) = &self.answered
            && !questions.iter().any(|q| q.id == answered.question.id)
        {
            let key = (answered.question.asked_at, answered.question.id);
            let at = questions
                .iter()
                .position(|q| (q.asked_at, q.id) > key)
                .unwrap_or(questions.len());
            questions.insert(at, answered.question.clone());
        }
        self.questions = questions;
        if self.selected_index().is_none() {
            let last = self.questions.len().checked_sub(1);
            if self.selected.is_some() {
                self.moved = true;
                self.end_opening();
            }
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

    /// キーの案内の代わりに出す 1 行。次にキーを押したら消す
    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
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

    pub fn set_answered_for(&mut self, delay: std::time::Duration) {
        self.answered_for = Some(delay);
    }

    pub fn answered_for(&self) -> std::time::Duration {
        self.answered_for.unwrap_or(super::ANSWERED_FOR)
    }

    pub fn set_answered(&mut self, question: Question, choice: Choice, until: Instant) {
        self.answered = Some(Answered {
            question,
            choice,
            until,
        });
    }

    /// 答えたばかりで残している質問なら、答えに使ったもの
    pub fn answered_with(&self, id: QuestionId) -> Option<&Choice> {
        self.answered
            .as_ref()
            .filter(|a| a.question.id == id)
            .map(|a| &a.choice)
    }

    /// 選んでいる質問に返事を打ち始める。打てない質問なら何もしない
    pub fn start_reply(&mut self) {
        if let Some(question) = self.selected()
            && matches!(question.body, QuestionBody::Ask { .. })
        {
            self.replying = Some(Replying {
                id: question.id,
                text: String::new(),
            });
        }
    }

    pub fn reply(&self) -> Option<(QuestionId, &str)> {
        self.replying.as_ref().map(|r| (r.id, r.text.as_str()))
    }

    /// 返事先が選択から外れたり選択が勝手に戻ったりしても、打ち続けたキーがコマンドとして効かないよう Esc まで枠を閉じない
    pub fn reply_gone(&self) -> bool {
        self.replying
            .as_ref()
            .is_some_and(|r| self.moved || self.selected().is_none_or(|q| q.id != r.id))
    }

    /// 改行やタブで 1 行の枠が崩れないよう、制御文字は空白にして足す。上限を超える分は捨てて知らせる
    pub fn type_reply(&mut self, text: &str) {
        if self.reply_gone() {
            return;
        }
        let Some(replying) = self.replying.as_mut() else {
            return;
        };
        let text = text.replace("\r\n", "\n");
        let room = MAX_REPLY.saturating_sub(replying.text.chars().count());
        let typed = text.chars().map(|c| if c.is_control() { ' ' } else { c });
        replying.text.extend(typed.clone().take(room));
        if typed.count() > room {
            self.notice = Some(format!("a reply is limited to {MAX_REPLY} characters"));
        }
    }

    pub fn erase_reply(&mut self) {
        if self.reply_gone() {
            return;
        }
        if let Some(replying) = self.replying.as_mut() {
            replying.text.pop();
        }
    }

    pub fn take_reply(&mut self) -> Option<(QuestionId, String)> {
        self.replying.take().map(|r| (r.id, r.text))
    }

    pub fn answered_until(&self) -> Option<Instant> {
        self.answered.as_ref().map(|a| a.until)
    }

    /// 期限が来ていれば手放し、一覧を取り直す
    pub fn expire_answered(&mut self, now: Instant, questions: Vec<Question>) {
        if self.answered_until().is_some_and(|until| until <= now) {
            self.answered = None;
        }
        self.set_questions(questions);
    }

    pub fn selected_is_answered(&self) -> bool {
        self.selected()
            .is_some_and(|q| self.answered_with(q.id).is_some())
    }

    /// 一覧が変わって選択が移ったまま、人がまだ選び直していない
    pub fn moved_on_its_own(&self) -> bool {
        self.moved
    }

    pub fn set_shown_buttons(&self, count: usize) {
        self.shown_buttons.set(count);
    }

    pub fn shown_buttons(&self) -> usize {
        self.shown_buttons.get()
    }

    pub fn set_shown_links(&self, count: usize) {
        self.shown_links.set(count);
    }

    pub fn shown_links(&self) -> usize {
        self.shown_links.get()
    }

    pub fn start_opening(&mut self) {
        self.opening = true;
        self.opened = None;
    }

    pub fn opening(&self) -> bool {
        self.opening
    }

    pub fn end_opening(&mut self) {
        self.opening = false;
        self.opened = None;
    }

    /// 同じ待ちの中で直前に開いたリンクなら false
    pub fn mark_opened(&mut self, link: usize) -> bool {
        self.opened.replace(link) != Some(link)
    }

    /// 人がクリックで選んだ。選び直し待ちを解く
    pub fn select(&mut self, id: QuestionId) {
        if self.questions.iter().any(|q| q.id == id) {
            self.selected = Some(id);
            self.moved = false;
        }
    }

    pub fn clear_targets(&self) {
        self.targets.borrow_mut().clear();
    }

    pub fn add_target(&self, area: Rect, target: Target) {
        self.targets.borrow_mut().push((area, target));
    }

    pub fn target_at(&self, column: u16, row: u16) -> Option<Target> {
        let position = Position::new(column, row);
        let targets = self.targets.borrow();
        targets
            .iter()
            .find(|(area, _)| area.contains(position))
            .map(|&(_, target)| target)
    }

    fn move_selection(&mut self, step: impl Fn(usize) -> usize) {
        let Some(current) = self.selected_index() else {
            return;
        };
        self.moved = false;
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
    fn marks_a_selection_that_moved_until_the_person_chooses() {
        let mut app = App::default();
        let [a, b] = [question("a"), question("b")];
        app.set_questions(vec![a.clone()]);
        assert!(!app.moved_on_its_own(), "the first selection is not a move");
        app.set_questions(vec![a.clone(), b.clone()]);
        assert!(!app.moved_on_its_own(), "kept");
        app.set_questions(vec![b.clone()]);
        assert!(app.moved_on_its_own());
        app.select_previous();
        assert!(!app.moved_on_its_own());

        app.set_questions(vec![]);
        app.set_questions(vec![a.clone()]);
        assert!(
            app.moved_on_its_own(),
            "an empty list in between keeps the mark"
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

    #[test]
    fn keeps_a_reply_on_one_line_and_within_the_limit() {
        let mut app = App::default();
        let q = question("w1:p1");
        app.set_questions(vec![q.clone()]);
        app.type_reply("ignored before r");
        assert_eq!(app.reply(), None);

        app.start_reply();
        app.type_reply("a\r\nb");
        assert_eq!(app.reply(), Some((q.id, "a b")));
        app.take_reply();
        app.start_reply();
        app.type_reply("a\tb\nc");
        app.erase_reply();
        assert_eq!(app.reply(), Some((q.id, "a b ")));
        app.type_reply(&"x".repeat(MAX_REPLY - 4));
        assert_eq!(app.notice(), None, "exactly at the limit");
        app.type_reply("y");
        assert_eq!(app.reply().unwrap().1.chars().count(), MAX_REPLY);
        assert!(app.notice().unwrap().contains("limited"));
    }

    #[test]
    fn opens_no_reply_for_a_permission_request() {
        let mut app = App::default();
        let mut q = question("w1:p1");
        q.body = QuestionBody::Permission {
            request_id: "abcde".into(),
            tool_name: "Bash".into(),
            description: "d".into(),
            input_preview: "p".into(),
        };
        app.set_questions(vec![q]);
        app.start_reply();
        assert_eq!(app.reply(), None);
    }

    #[test]
    fn keeps_the_reply_box_when_its_question_goes_away() {
        let mut app = App::default();
        let q = question("w1:p1");
        app.set_questions(vec![q.clone()]);
        app.start_reply();
        app.type_reply("half");
        app.set_questions(vec![q.clone(), question("w1:p2")]);
        assert!(app.reply().is_some(), "another question arriving keeps it");
        app.set_questions(vec![]);
        assert!(
            app.reply_gone(),
            "the box stays so the typed keys are not commands"
        );
        app.type_reply("q");
        app.erase_reply();
        assert_eq!(app.reply(), Some((q.id, "half")));
        assert_eq!(app.notice(), None);
        app.set_questions(vec![q.clone()]);
        assert!(
            app.reply_gone(),
            "asked again, it still waits for Esc like the buttons"
        );
    }
}
