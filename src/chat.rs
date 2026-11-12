//! 왼쪽에 고정된 대화 구역: 주고받은 말, 진행 표시, 입력창, 버튼.

use eframe::egui;

use crate::{App, llm, sys, ui, vault};

#[allow(non_snake_case)]
fn PANEL() -> egui::Color32 { ui::ZONE() }
#[allow(non_snake_case)]
fn PANEL_EDGE() -> egui::Color32 { ui::ZONE_EDGE() }
#[allow(non_snake_case)]
fn FIELD() -> egui::Color32 { if ui::is_light() { egui::Color32::WHITE } else { egui::Color32::from_rgb(28, 29, 32) } }
#[allow(non_snake_case)]
fn FIELD_EDGE() -> egui::Color32 { if ui::is_light() { egui::Color32::from_rgb(210, 210, 215) } else { egui::Color32::from_rgb(50, 51, 56) } }
pub const WIDTH: f32 = 444.0;

/// 이름이 붙은 스위치. 손잡이가 오른쪽에 밝게 있으면 켜진 것, 왼쪽에 어둡게 있으면 꺼진 것. 눌렸으면 true.
fn switch(ui: &mut egui::Ui, ctx: &egui::Context, id: &str, label: &str, on: bool, hint: &str) -> bool {
    let (r, p) = ui.allocate_painter(egui::vec2(104.0, 40.0), egui::Sense::click());
    let k = ui::ease_in_out_cubic(ctx.animate_bool_with_time(ui.scope_id().with(id), on, 0.28));
    let lerp = |a: egui::Color32, b: egui::Color32| {
        let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k) as u8;
        egui::Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
    };
    p.rect_filled(r.rect, 20.0, FIELD());
    p.rect_stroke(r.rect, 20.0, egui::Stroke::new(1.0, if r.hovered() { ui::DIM() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
    p.text(r.rect.left_center() + egui::vec2(18.0, 0.0), egui::Align2::LEFT_CENTER, label, ui::bold(13.0), lerp(ui::DIM(), ui::TEXT()));
    let track = egui::Rect::from_center_size(r.rect.right_center() - egui::vec2(32.0, 0.0), egui::vec2(36.0, 20.0));
    p.rect_filled(track, 10.0, lerp(if ui::is_light() { egui::Color32::from_rgb(208, 208, 213) } else { egui::Color32::from_rgb(52, 53, 58) }, ui::TEXT()));
    p.circle_filled(egui::pos2(track.left() + 10.0 + 16.0 * k, track.center().y), 7.0, lerp(ui::DIM(), FIELD()));
    r.on_hover_text(hint).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

use eframe::egui::Rect;

/// 도구가 하는 일을 사용자가 알아볼 말로 바꾼다. 명령어나 긴 경로를 그대로 보여 주지 않는다.
fn plain(activity: &str) -> String {
    let (kind, detail) = activity.split_once(": ").unwrap_or((activity, ""));
    // 경로는 파일 이름만
    let name = detail.trim_end_matches('…').rsplit(['\\', '/']).next().unwrap_or("").trim();
    let short = |s: &str| s.chars().take(24).collect::<String>();
    let with = |what: &str, extra: &str| if extra.is_empty() { what.to_string() } else { format!("{what} · {}", short(extra)) };
    match kind {
        "명령 실행" => "컴퓨터에서 작업하는 중".to_string(),
        "파일 읽기" => with("파일을 읽는 중", name),
        "파일 쓰기" => with("파일을 만드는 중", name),
        "파일 고치기" => with("파일을 고치는 중", name),
        "폴더 보기" => with("폴더를 살펴보는 중", name),
        "글자 찾기" => "파일에서 찾는 중".to_string(),
        "기억 찾기" | "기억 읽기" => "기억을 찾아보는 중".to_string(),
        "기억 쓰기" | "기억 덧붙이기" => "기억해 두는 중".to_string(),
        "페이지 읽기" | "페이지 조작" => "페이지를 살펴보는 중".to_string(),
        "웹 검색" => with("웹에서 찾는 중", detail),
        "웹 페이지 읽기" => "웹 페이지를 읽는 중".to_string(),
        "작업 맡기기" => with("작업자에게 맡기는 중", detail),
        "작업자에게 전하기" | "작업 멈추기" | "작업 결과 보기" | "맡긴 작업 확인" => "맡긴 작업을 확인하는 중".to_string(),
        "장소 찾기" => with("장소를 찾는 중", detail),
        "길 찾기" => "길을 찾는 중".to_string(),
        "그림 보기" => "그림을 보는 중".to_string(),
        "보드 읽기" => "화면 내용을 확인하는 중".to_string(),
        _ => activity.to_string(),
    }
}

impl App {
    pub fn chat_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, talking: bool) {
        egui::Panel::left("chat")
            .exact_size(WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(ui::BG()).inner_margin(egui::Margin { left: 18, right: 0, top: 18, bottom: 18 }))
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(PANEL())
                    .stroke(egui::Stroke::new(1.0, PANEL_EDGE()))
                    .corner_radius(26)
                    .inner_margin(egui::Margin::same(18))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        // 내가 할 일이 있으면 이 구역을 위아래로 반씩 나눈다: 위는 "내 차례"(지금 처리할 일 하나), 아래는 대화.
                        let whole = ui.available_rect_before_wrap();
                        let asks = self.board.cards.iter().filter(|c| c.kind.asks()).count();
                        let lower = if asks > 0 {
                            let (top, bottom) = whole.split_top_bottom_at_fraction(0.5);
                            ui.scope_builder(egui::UiBuilder::new().max_rect(top), |ui| self.task_slot(ui, asks));
                            Rect::from_min_max(bottom.min + egui::vec2(0.0, 14.0), bottom.max)
                        } else {
                            self.view.task_rect = None;
                            whole
                        };
                        ui.scope_builder(egui::UiBuilder::new().max_rect(lower), |ui| {
                        ui.set_min_size(ui.available_size());
                        // 아래에서 위로 쌓는다: 맨 아래에 상태 줄, 그 위에 버튼, 입력창, 진행 표시. 남는 위쪽이 대화.
                        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                            self.reset_row(ui);
                            ui.label(egui::RichText::new(self.status_line()).size(10.5).color(ui::FAINT()));
                            ui.add_space(8.0);
                            self.buttons(ui, ctx);
                            ui.add_space(8.0);
                            self.shelf_row(ui);
                            if self.cfg.key.is_none() && std::env::var("LYBOARD_DEMO").is_err() {
                                self.key_setup(ui);
                            } else {
                                self.input_box(ui, ctx);
                                self.pics_row(ui);
                            }
                            ui.add_space(8.0);
                            if let Some(e) = self.error.clone() {
                                ui.label(egui::RichText::new(e).size(12.5).color(ui::RED()));
                                ui.add_space(4.0);
                            }
                            self.progress(ui, ctx, talking);
                            ui.add_space(6.0);
                            self.history(ui);
                        });
                        });
                    });
            });
    }

    /// "내 차례": 내가 답하거나 승인해야 할 일이 하나씩 여기로 온다. 처리하면 다음 일이 온다.
    /// 여기서는 머리 줄만 그리고 카드 자리를 잡아 둔다. 카드는 보드가 그 자리에 그린다.
    fn task_slot(&mut self, ui: &mut egui::Ui, asks: usize) {
        let now = ui.input(|i| i.time);
        let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
        let pulse = ((now * 2.4).sin() * 0.5 + 0.5) as f32;
        let dot = r.rect.left_center() + egui::vec2(8.0, 0.0);
        p.circle_filled(dot, 4.0 + 3.0 * pulse, ui::ORANGE().gamma_multiply(0.25 * (1.0 - pulse)));
        p.circle_filled(dot, 4.0, ui::ORANGE());
        let tr = p.text(r.rect.left_center() + egui::vec2(22.0, 0.0), egui::Align2::LEFT_CENTER, "내 차례", ui::bold(15.0), ui::TEXT());
        // 남은 일의 수. 지금 앞에 있는 것까지 센다.
        p.text(egui::pos2(tr.right() + 10.0, tr.center().y), egui::Align2::LEFT_CENTER, format!("남은 {asks}건"), ui::bold(12.5), ui::ORANGE());
        // 다른 일부터 보고 싶으면 넘긴다.
        if asks > 1 {
            let br = Rect::from_min_size(egui::pos2(r.rect.right() - 62.0, r.rect.top() + 2.0), egui::vec2(60.0, 26.0));
            let b = ui.interact(br, ui.scope_id().with("task-next"), egui::Sense::click());
            p.rect_stroke(br, 13.0, egui::Stroke::new(1.0, if b.hovered() { ui::DIM() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
            p.text(br.center(), egui::Align2::CENTER_CENTER, "다음 ›", ui::bold(12.0), if b.hovered() { ui::TEXT() } else { ui::DIM() });
            if b.on_hover_text("이 일은 나중에 하고 다음 일부터 봅니다").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.next_task();
            }
        }
        ui.add_space(8.0);
        let rest = ui.available_rect_before_wrap();
        // 아래 대화 구역과의 경계선
        ui.painter().hline(rest.x_range(), rest.bottom() + 6.0, egui::Stroke::new(1.0, PANEL_EDGE()));
        self.view.task_rect = Some(rest);
        if self.view.task.is_none() {
            // 방금 한 답을 반영하는 중: 끝나면 다음 일이 이 자리로 온다.
            ui.painter().text(rest.left_top() + egui::vec2(4.0, 10.0), egui::Align2::LEFT_TOP, "답을 반영하는 중입니다. 끝나면 다음 일이 옵니다.", ui::font(12.5), ui::FAINT());
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50)); // 점이 깜빡인다
    }

    /// 맨 아래의 초기화. 실수로 지우지 않게 두 번 눌러야 한다: 한 번 누르면 확인을 묻고, 6초 안에 다시 눌러야 지운다.
    fn reset_row(&mut self, ui: &mut egui::Ui) {
        let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 20.0), egui::Sense::hover());
        // 왼쪽 끝: 밝은 화면 / 어두운 화면
        let t = Rect::from_min_size(r.rect.left_top(), egui::vec2(60.0, 20.0));
        let tr = ui.interact(t, ui.scope_id().with("theme"), egui::Sense::click());
        let label = if ui::is_light() { "어둡게" } else { "밝게" };
        p.text(t.left_center(), egui::Align2::LEFT_CENTER, label, ui::font(11.0), if tr.hovered() { ui::TEXT() } else { ui::FAINT() });
        if tr.on_hover_text("밝은 화면과 어두운 화면을 바꿉니다").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            crate::set_theme(ui.ctx(), !ui::is_light(), true);
            self.titlebar_done = false;
        }
        // 그 옆: 대화 언어 (AI 의 대답과 받아쓰기)
        let l = Rect::from_min_size(egui::pos2(t.right() + 4.0, r.rect.top()), egui::vec2(60.0, 20.0));
        let lr = ui.interact(l, ui.scope_id().with("lang"), egui::Sense::click());
        let label = if crate::sys::english() { "한국어" } else { "English" };
        p.text(l.left_center(), egui::Align2::LEFT_CENTER, label, ui::font(11.0), if lr.hovered() { ui::TEXT() } else { ui::FAINT() });
        if lr
            .on_hover_text(if crate::sys::english() { "대화를 한국어로 바꿉니다" } else { "Talk in English: answers, cards and voice" })
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            crate::sys::set_english(!crate::sys::english(), true);
        }
        if self.cfg.demo() {
            ui.add_space(2.0);
            return;
        }
        let armed = self.reset_armed.map(|t| t.elapsed().as_secs() < 6).unwrap_or(false);
        if !armed {
            self.reset_armed = None;
        }
        if armed {
            // 왼쪽에 경고, 오른쪽에 두 버튼. 서로 겹치지 않게 경고 글은 버튼 왼쪽까지만 그린다.
            let yes = Rect::from_min_size(egui::pos2(r.rect.right() - 118.0, r.rect.top()), egui::vec2(66.0, 20.0));
            let no = Rect::from_min_size(egui::pos2(r.rect.right() - 46.0, r.rect.top()), egui::vec2(46.0, 20.0));
            let warn = p.with_clip_rect(Rect::from_min_max(r.rect.min, egui::pos2(yes.left() - 8.0, r.rect.bottom())));
            warn.text(r.rect.left_center(), egui::Align2::LEFT_CENTER, "화면·대화·기억·기록을 전부 지웁니다", ui::bold(11.5), ui::RED());
            let y = ui.interact(yes, ui.scope_id().with("reset-yes"), egui::Sense::click());
            let n = ui.interact(no, ui.scope_id().with("reset-no"), egui::Sense::click());
            p.rect_stroke(yes, 10.0, egui::Stroke::new(1.0, ui::RED()), egui::StrokeKind::Inside);
            p.text(yes.center(), egui::Align2::CENTER_CENTER, "지우기", ui::bold(11.5), ui::RED());
            p.text(no.center(), egui::Align2::CENTER_CENTER, "취소", ui::bold(11.5), if n.hovered() { ui::TEXT() } else { ui::DIM() });
            if y.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.reset_armed = None;
                self.reset_all();
            } else if n.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.reset_armed = None;
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(300));
        } else {
            let b = Rect::from_min_size(egui::pos2(r.rect.right() - 46.0, r.rect.top()), egui::vec2(46.0, 20.0));
            let resp = ui.interact(b, ui.scope_id().with("reset"), egui::Sense::click());
            p.text(b.right_center(), egui::Align2::RIGHT_CENTER, "초기화", ui::font(11.0), if resp.hovered() { ui::TEXT() } else { ui::FAINT() });
            if resp.on_hover_text("화면, 대화, 기억, 기록을 전부 지우고 처음부터 시작합니다 (API 키와 설정은 남습니다)").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.reset_armed = Some(std::time::Instant::now());
            }
        }
        ui.add_space(2.0);
    }

    /// 보관함으로 가는 한 줄. 보관된 주제가 있을 때만 보인다.
    fn shelf_row(&mut self, ui: &mut egui::Ui) {
        let n = self.board.shelf.len();
        if n == 0 {
            self.show_shelf = false;
            return;
        }
        let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 24.0), egui::Sense::click());
        let label = if self.show_shelf { "← 대화로 돌아가기".to_string() } else { format!("보관된 주제 {n}개  ›") };
        p.text(r.rect.left_center() + egui::vec2(4.0, 0.0), egui::Align2::LEFT_CENTER, label, ui::bold(12.5), if r.hovered() { ui::TEXT() } else { ui::DIM() });
        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.show_shelf = !self.show_shelf;
        }
        ui.add_space(4.0);
    }

    /// 보관함 목록. 누르면 그 주제가 화면으로 돌아온다.
    fn shelf_list(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let mut back = None;
        egui::ScrollArea::vertical().auto_shrink(false).max_height(ui.available_height()).show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(width);
                ui.label(egui::RichText::new("보관된 주제 · 누르면 화면으로 다시 꺼냅니다").size(12.0).color(ui::FAINT()));
                ui.add_space(8.0);
                for s in self.board.shelf.iter().rev() {
                    let (r, p) = ui.allocate_painter(egui::vec2(width - 6.0, 46.0), egui::Sense::click());
                    p.rect_filled(r.rect, 14.0, FIELD());
                    p.rect_stroke(r.rect, 14.0, egui::Stroke::new(1.0, if r.hovered() { ui::DIM() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
                    // 이름이 길어도 줄 밖으로 나가지 않게 이 줄 안에서만 그린다.
                    let inner = p.with_clip_rect(r.rect.shrink2(egui::vec2(12.0, 0.0)));
                    inner.text(r.rect.left_top() + egui::vec2(14.0, 8.0), egui::Align2::LEFT_TOP, &s.topic.name, ui::bold(13.5), ui::TEXT());
                    let titles: Vec<&str> = s.cards.iter().take(4).map(|c| c.title.as_str()).collect();
                    inner.text(r.rect.left_top() + egui::vec2(14.0, 27.0), egui::Align2::LEFT_TOP, format!("카드 {}장 · {}", s.cards.len(), titles.join(", ")), ui::font(11.0), ui::FAINT());
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        back = Some(s.topic.id.clone());
                    }
                    ui.add_space(6.0);
                }
            });
        });
        if let Some(id) = back {
            self.apply_line(&format!("restore {id}"), crate::core::Author::User);
            self.view.want_topic(id);
            if self.board.shelf.is_empty() {
                self.show_shelf = false;
            }
        }
    }

    /// 주고받은 말. 화면에 글을 쌓지 않는 것이 이 프로그램의 원칙이므로, AI 쪽은 한마디(say)만 남는다.
    fn history(&mut self, ui: &mut egui::Ui) {
        if self.show_shelf && !self.board.shelf.is_empty() {
            return self.shelf_list(ui);
        }
        let width = ui.available_width();
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).max_height(ui.available_height()).show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(width);
                let mine = |ui: &mut egui::Ui, text: &str| {
                    ui.label(egui::RichText::new(text).size(13.0).color(ui::DIM()));
                };
                let yours = |ui: &mut egui::Ui, text: &str, strong: bool| {
                    let t = egui::RichText::new(text).font(ui::bold(15.0)).color(if strong { ui::TEXT() } else { ui::DIM() });
                    ui.label(t);
                    ui.add_space(14.0);
                };
                let n = self.recent.len();
                if n == 0 && !self.busy {
                    if self.board.say.is_empty() {
                        ui.label(egui::RichText::new("무엇이든 말하세요.\n답은 오른쪽 화면에 카드로 나타납니다.").size(13.5).color(ui::FAINT()));
                    } else {
                        yours(ui, &self.board.say.clone(), true);
                    }
                }
                for (i, (u, a)) in self.recent.iter().enumerate() {
                    mine(ui, u);
                    ui.add_space(3.0);
                    yours(ui, a, i + 1 == n && !self.busy);
                }
                // 답하는 중에 더 적은 말: 아직 전해지지 않은 것
                let waiting: Vec<String> = self.queue.lock().map(|q| q.clone()).unwrap_or_default();
                if self.busy {
                    mine(ui, &self.cur_input.clone());
                    ui.add_space(3.0);
                    // 지금 오고 있는 답: 말하는 중이면 그 문장, 아니면 흘러 들어오는 한마디
                    let live = self
                        .caption
                        .clone()
                        .or_else(|| self.say_live.clone())
                        .or_else(|| self.turn_said.then(|| self.board.say.clone()))
                        // 아직 한마디가 없는데 일은 시작했으면, 말없이 일하는 것처럼 보이지 않게 한다.
                        .or_else(|| (!self.activity.is_empty() && !self.tidying).then(|| "확인하고 있어요.".to_string()));
                    if let Some(text) = live.filter(|t| !t.is_empty()) {
                        yours(ui, &text, true);
                    }
                } else if let Some(c) = self.caption.clone() {
                    // 읽고 있는 문장이 방금 한 말과 같으면 두 번 보이지 않게 한다.
                    if self.recent.last().map(|(_, a)| a.trim() != c.trim()).unwrap_or(true) {
                        yours(ui, &c, true);
                    }
                }
                if self.standby {
                    ui.label(egui::RichText::new("기다리는 중 · 부르거나 하던 얘기로 돌아오면 다시 들어요").size(13.0).color(ui::FAINT()));
                    ui.add_space(3.0);
                }
                for w in &waiting {
                    ui.label(egui::RichText::new(format!("곧 전함 · {w}")).size(13.0).color(ui::FAINT()));
                    ui.add_space(3.0);
                }
            });
        });
    }

    /// 진행 표시: 지금 무엇을 하고 있는지와 흐른 시간. 한가할 때는 자리를 차지하지 않는다.
    fn progress(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, talking: bool) {
        let now = ui.input(|i| i.time);
        let listening = self.mic.is_some();
        let writing_down = self.stt.is_some();
        let active = self.busy || talking || self.react_at.is_some() || listening || writing_down;
        if !active {
            return;
        }
        let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 22.0), egui::Sense::hover());
        let dot = r.rect.left_center() + egui::vec2(8.0, 0.0);
        if listening && !self.busy {
            // 듣는 중: 들어오는 소리의 크기만큼 고리가 커진다.
            let level = self.mic.as_ref().map(|m| m.level()).unwrap_or(0.0);
            let size = (level * 60.0).clamp(0.0, 7.0);
            p.circle_stroke(dot, 4.0 + size, egui::Stroke::new(1.3, ui::TEXT().gamma_multiply(0.6)));
            p.circle_filled(dot, 3.0, ui::TEXT());
            let text = if self.heard { "듣는 중 · 말이 끝나면 알아서 보냅니다" } else { "듣는 중 · 말씀하세요" };
            p.with_clip_rect(r.rect).text(r.rect.left_center() + egui::vec2(24.0, 0.0), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(12.5), ui::TEXT());
            ctx.request_repaint();
            return;
        }
        if talking {
            // 말하는 중: 목소리처럼 일렁이는 고리
            for (i, speed) in [5.3_f64, 7.9, 3.7].iter().enumerate() {
                let wave = ((now * speed + i as f64 * 1.3).sin() * 0.5 + 0.5) as f32;
                p.circle_stroke(dot, 3.5 + i as f32 * 2.0 + wave * 2.0, egui::Stroke::new(1.1, ui::TEXT().gamma_multiply(0.75 - i as f32 * 0.22)));
            }
            p.circle_filled(dot, 3.0, ui::TEXT());
        } else {
            let k = ui::ease_out_cubic((now * 0.9).fract() as f32);
            p.circle_stroke(dot, 3.0 + 6.0 * k, egui::Stroke::new(1.2, ui::ORANGE().gamma_multiply(0.6 * (1.0 - k))));
            p.circle_filled(dot, 3.0, ui::ORANGE());
        }
        let what = if !self.busy {
            if talking {
                "말하는 중".to_string()
            } else if writing_down {
                "받아쓰는 중".to_string()
            } else {
                "받았어요".to_string()
            }
        } else if !self.activity.is_empty() {
            plain(&self.activity)
        } else if !self.got_output {
            match self.pre.as_ref().and_then(|p| p.kind.clone()) {
                Some(kind) => format!("{kind} · 생각하는 중"),
                None if self.tidying => "분류와 주제 정리 중".to_string(),
                None => "생각하는 중".to_string(),
            }
        } else {
            // 지금 쓰고 있는 카드와, 이번 턴에 그 카드에 넣은 항목 수
            let last_card = self.turn_raw.lines().rev().find_map(|l| {
                let mut w = l.split_whitespace();
                matches!(w.next(), Some("card" | "set" | "add" | "upd" | "del")).then(|| w.next().unwrap_or("").to_string())
            });
            match last_card.and_then(|id| self.board.card(&id).map(|c| (id, c.title.clone()))) {
                Some((id, title)) => {
                    let n = self.turn_raw.lines().filter(|l| l.starts_with(&format!("add {id} ")) || l.starts_with(&format!("upd {id} "))).count();
                    let title: String = title.chars().take(18).collect();
                    if n > 0 { format!("'{title}' 쓰는 중 · {n}줄") } else { format!("'{title}' 쓰는 중") }
                }
                None => "화면에 쓰는 중".to_string(),
            }
        };
        let text = if self.busy { format!("{what} · {}초", self.sent_at.elapsed().as_secs()) } else { what };
        // 멈춤: 도는 턴을 그만두게 한다. 글자는 이 버튼 왼쪽까지만 그려서 겹치지 않게 한다.
        let mut right = r.rect.right();
        if self.busy {
            let stopping = self.stop.load(std::sync::atomic::Ordering::Relaxed);
            let b = Rect::from_min_size(egui::pos2(r.rect.right() - 50.0, r.rect.top()), egui::vec2(50.0, 22.0));
            right = b.left() - 8.0;
            let resp = ui.interact(b, ui.scope_id().with("stop-turn"), egui::Sense::click());
            p.rect_stroke(b, 11.0, egui::Stroke::new(1.0, if resp.hovered() { ui::TEXT() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
            p.text(b.center(), egui::Align2::CENTER_CENTER, if stopping { "멈추는 중" } else { "멈춤" }, ui::bold(if stopping { 10.5 } else { 11.5 }), if resp.hovered() { ui::TEXT() } else { ui::DIM() });
            if resp.on_hover_text("지금 하는 일을 그만두게 합니다").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let clip = p.with_clip_rect(Rect::from_min_max(r.rect.min, egui::pos2(right, r.rect.bottom())));
        clip.text(r.rect.left_center() + egui::vec2(24.0, 0.0), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(12.5), ui::ORANGE());
        ctx.request_repaint();
    }

    /// 입력창 위: 붙여 둔 사진들. 누르면 뺀다.
    fn pics_row(&mut self, ui: &mut egui::Ui) {
        if self.pics.is_empty() {
            return;
        }
        let mut remove = None;
        ui.add_space(6.0);
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 58.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for (n, (pic, tex)) in self.pics.iter().enumerate() {
                let (r, p) = ui.allocate_painter(egui::vec2(56.0, 56.0), egui::Sense::click());
                // 그림의 가운데를 정사각형으로 잘라 보여 준다.
                let (w, h) = (pic.thumb.0 as f32, pic.thumb.1 as f32);
                let side = w.min(h);
                let uv = Rect::from_min_max(egui::pos2((w - side) / 2.0 / w, (h - side) / 2.0 / h), egui::pos2((w + side) / 2.0 / w, (h + side) / 2.0 / h));
                egui::Image::new((tex.id(), egui::vec2(56.0, 56.0))).uv(uv).corner_radius(12).paint_at(ui, r.rect);
                p.rect_stroke(r.rect, 12.0, egui::Stroke::new(1.0, if r.hovered() { ui::TEXT() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
                if r.hovered() {
                    p.rect_filled(r.rect, 12.0, egui::Color32::from_black_alpha(140));
                    p.text(r.rect.center(), egui::Align2::CENTER_CENTER, "빼기", ui::bold(12.0), ui::TEXT());
                }
                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    remove = Some(n);
                }
            }
        });
        if let Some(n) = remove {
            let _ = self.pics.remove(n);
        }
    }

    fn input_box(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let now = ui.input(|i| i.time);
        let pill = egui::Frame::new()
            .fill(FIELD())
            .stroke(egui::Stroke::new(1.0, FIELD_EDGE()))
            .corner_radius(20)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.input)
                        .hint_text(if self.busy { "답하는 중에도 적으면 이어서 전합니다" } else { "무엇이든 말하세요 · 사진은 붙여넣거나 끌어다 놓으세요" })
                        .desired_width(f32::INFINITY)
                        .desired_rows(2)
                        // Enter 는 보내기, 줄을 바꾸려면 Shift+Enter
                        .return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter))
                        .font(egui::FontId::proportional(15.0))
                        .text_color(ui::TEXT())
                        .frame(egui::Frame::NONE),
                )
            });
        let edit = pill.inner;
        // 보낸 순간부터 답이 끝날 때까지 입력창 테두리에 주황색 선이 돈다.
        let run = ctx.animate_bool_with_time(ui.scope_id().with("input-busy"), self.busy, 0.3);
        if run > 0.01 {
            ui::draw_runner(ui.painter(), pill.response.rect, 20.0, now, run);
            ctx.request_repaint();
        }
        if self.focus_input {
            edit.request_focus();
            self.focus_input = false;
        }
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        if edit.has_focus() && enter {
            self.send(ctx);
            self.focus_input = true;
        }
    }

    /// "기록" 버튼과 음성 스위치.
    fn buttons(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut toggle = None;
        let mut talk = None;
        // 높이를 정해 주지 않으면 이 줄이 남은 세로 공간을 전부 차지한다.
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // 기록: 오늘의 기록 노트를 옵시디언에서 연다 (옵시디언이 없으면 보관함 폴더를 연다).
            // 사진: 파일을 골라 붙인다 (붙여넣기와 끌어다 놓기도 된다).
            let (pr, pp) = ui.allocate_painter(egui::vec2(64.0, 40.0), egui::Sense::click());
            pp.rect_filled(pr.rect, 20.0, FIELD());
            pp.rect_stroke(pr.rect, 20.0, egui::Stroke::new(1.0, if pr.hovered() { ui::DIM() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
            pp.text(pr.rect.center(), egui::Align2::CENTER_CENTER, "사진", ui::bold(13.0), if pr.hovered() { ui::TEXT() } else { ui::DIM() });
            if pr.on_hover_text("사진 파일을 골라 붙입니다 · 캡처한 그림은 Ctrl+V 로 바로 붙습니다").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                let picked = rfd::FileDialog::new().add_filter("사진", &["png", "jpg", "jpeg", "webp", "gif", "bmp"]).pick_files().unwrap_or_default();
                for path in picked {
                    self.add_pic(ctx, crate::pics::from_file(&path.display().to_string()));
                }
            }
            let (nr, np) = ui.allocate_painter(egui::vec2(64.0, 40.0), egui::Sense::click());
            np.rect_filled(nr.rect, 20.0, FIELD());
            np.rect_stroke(nr.rect, 20.0, egui::Stroke::new(1.0, if nr.hovered() { ui::DIM() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
            np.text(nr.rect.center(), egui::Align2::CENTER_CENTER, "기록", ui::bold(13.0), if nr.hovered() { ui::TEXT() } else { ui::DIM() });
            if nr.on_hover_text("오늘의 기록과 기억 보관함 열기").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                let (date, _) = sys::now_local();
                let today = vault::root().join("기록").join(format!("{date}.md"));
                sys::open_note(&vault::root(), &today);
            }
            // 음성: AI 의 답을 소리로 듣는다.
            let hint = if self.voice { "음성 켜짐 · 누르면 끕니다" } else { "음성 꺼짐 · 누르면 켭니다" };
            if switch(ui, ctx, "voice-toggle", "음성", self.voice, hint) {
                toggle = Some(!self.voice);
            }
            // 대화: 마이크로 말해서 시킨다. 말이 끝나면 알아서 보내고, 답을 다 들으면 다시 듣는다.
            let hint = if crate::eleven::key().is_none() {
                "말로 대화하려면 ElevenLabs 키가 필요합니다 (프로그램 폴더의 eleven.txt)"
            } else if self.converse {
                "말로 대화하는 중 · 누르면 마이크를 끕니다"
            } else {
                "누르면 마이크를 켜고 말로 대화합니다"
            };
            if switch(ui, ctx, "mic-toggle", "대화", self.converse, hint) && crate::eleven::key().is_some() {
                talk = Some(!self.converse);
            }
        });
        if let Some(on) = toggle {
            self.set_voice(on);
            if !on {
                self.converse = false; // 소리를 끄면 말로 하는 대화도 끝낸다
            }
        }
        if let Some(on) = talk {
            self.converse = on;
            if on && !self.voice {
                self.set_voice(true); // 말로 대화할 때는 답도 소리로 듣는다
            }
        }
    }

    /// 처음 쓰는 사람을 위한 화면: API 키를 넣으면 프로그램 폴더에 저장하고 바로 시작한다.
    fn key_setup(&mut self, ui: &mut egui::Ui) {
        let field = |ui: &mut egui::Ui, value: &mut String, hint: &str| {
            egui::Frame::new().fill(FIELD()).stroke(egui::Stroke::new(1.0, FIELD_EDGE())).corner_radius(14).inner_margin(egui::Margin::symmetric(14, 9)).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(value)
                        .password(true)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY)
                        .font(egui::FontId::proportional(13.5))
                        .text_color(ui::TEXT())
                        .frame(egui::Frame::NONE),
                )
            });
        };
        // 아래에서 위로 쌓이므로 버튼이 먼저 온다.
        let ready = self.key_input.trim().len() > 20;
        let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
        p.rect_filled(r.rect, 20.0, if ready { ui::TEXT() } else { FIELD() });
        p.text(r.rect.center(), egui::Align2::CENTER_CENTER, "저장하고 시작", ui::bold(13.5), if ready { PANEL() } else { ui::FAINT() });
        if ready && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            let base = sys::base_dir();
            let _ = std::fs::write(base.join("key.txt"), self.key_input.trim());
            if !self.jev_input.trim().is_empty() {
                let _ = std::fs::write(base.join("jev.txt"), self.jev_input.trim());
            }
            self.key_input.clear();
            self.jev_input.clear();
            self.cfg = llm::Config::load();
            self.start_real_mode();
        }
        ui.add_space(8.0);
        field(ui, &mut self.jev_input, "TypeSafe Jev 키 (선택 · 없어도 됩니다)");
        ui.add_space(6.0);
        field(ui, &mut self.key_input, "Claude API 키 (sk-ant-…)");
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("처음이시군요. 본인의 Claude API 키를 넣어 주세요.\n키는 이 프로그램 폴더의 key.txt 에만 저장되고 다른 곳으로 보내지지 않습니다.")
                .size(12.5)
                .color(ui::DIM()),
        );
    }
}
