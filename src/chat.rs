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
        "명령 실행" => crate::sys::tr("컴퓨터에서 작업하는 중", "Working on the computer").to_string(),
        "파일 읽기" => with(crate::sys::tr("파일을 읽는 중", "Reading a file"), name),
        "파일 쓰기" => with(crate::sys::tr("파일을 만드는 중", "Writing a file"), name),
        "파일 고치기" => with(crate::sys::tr("파일을 고치는 중", "Editing a file"), name),
        "폴더 보기" => with(crate::sys::tr("폴더를 살펴보는 중", "Looking through a folder"), name),
        "글자 찾기" => crate::sys::tr("파일에서 찾는 중", "Searching files").to_string(),
        "기억 찾기" | "기억 읽기" => crate::sys::tr("기억을 찾아보는 중", "Checking memory").to_string(),
        "기억 쓰기" | "기억 덧붙이기" => crate::sys::tr("기억해 두는 중", "Saving to memory").to_string(),
        "페이지 읽기" | "페이지 조작" => crate::sys::tr("페이지를 살펴보는 중", "Looking at the page").to_string(),
        "웹 검색" => with(crate::sys::tr("웹에서 찾는 중", "Searching the web"), detail),
        "웹 페이지 읽기" => crate::sys::tr("웹 페이지를 읽는 중", "Reading a web page").to_string(),
        "작업 맡기기" => with(crate::sys::tr("작업자에게 맡기는 중", "Handing off to a worker"), detail),
        "작업자에게 전하기" | "작업 멈추기" | "작업 결과 보기" | "맡긴 작업 확인" => crate::sys::tr("맡긴 작업을 확인하는 중", "Checking delegated work").to_string(),
        "장소 찾기" => with(crate::sys::tr("장소를 찾는 중", "Finding a place"), detail),
        "길 찾기" => crate::sys::tr("길을 찾는 중", "Finding the route").to_string(),
        "그림 보기" => crate::sys::tr("그림을 보는 중", "Looking at a picture").to_string(),
        "보드 읽기" => crate::sys::tr("화면 내용을 확인하는 중", "Checking the board").to_string(),
        "웹 검색 중" => crate::sys::tr(crate::sys::tr("웹에서 찾는 중", "Searching the web"), "Searching the web").to_string(),
        "웹 페이지 읽는 중" => crate::sys::tr(crate::sys::tr("웹 페이지를 읽는 중", "Reading a web page"), "Reading a web page").to_string(),
        // 영어 화면에서는 이름을 모르는 일도 한국어로 내보이지 않는다.
        _ if crate::sys::english() => "Working".to_string(),
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
                        // 내가 할 일이 있으면 이 구역을 위아래로 반씩 나눈다: 위는 crate::sys::tr("내 차례", "My turn")(지금 처리할 일 하나), 아래는 대화.
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
                            self.input_box(ui, ctx);
                            self.pics_row(ui);
                            if self.cfg.demo() && std::env::var("LYBOARD_DEMO").is_err() {
                                self.demo_row(ui);
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

    /// crate::sys::tr("내 차례", "My turn"): 내가 답하거나 승인해야 할 일이 하나씩 여기로 온다. 처리하면 다음 일이 온다.
    /// 여기서는 머리 줄만 그리고 카드 자리를 잡아 둔다. 카드는 보드가 그 자리에 그린다.
    fn task_slot(&mut self, ui: &mut egui::Ui, asks: usize) {
        let now = ui.input(|i| i.time);
        let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
        let pulse = ((now * 2.4).sin() * 0.5 + 0.5) as f32;
        let dot = r.rect.left_center() + egui::vec2(8.0, 0.0);
        p.circle_filled(dot, 4.0 + 3.0 * pulse, ui::ORANGE().gamma_multiply(0.25 * (1.0 - pulse)));
        p.circle_filled(dot, 4.0, ui::ORANGE());
        let tr = p.text(r.rect.left_center() + egui::vec2(22.0, 0.0), egui::Align2::LEFT_CENTER, crate::sys::tr("내 차례", "My turn"), ui::bold(15.0), ui::TEXT());
        // 남은 일의 수. 지금 앞에 있는 것까지 센다.
        p.text(egui::pos2(tr.right() + 10.0, tr.center().y), egui::Align2::LEFT_CENTER, if crate::sys::english() { format!("{asks} left") } else { format!("남은 {asks}건") }, ui::bold(12.5), ui::ORANGE());
        // 다른 일부터 보고 싶으면 넘긴다.
        if asks > 1 {
            let br = Rect::from_min_size(egui::pos2(r.rect.right() - 62.0, r.rect.top() + 2.0), egui::vec2(60.0, 26.0));
            let b = ui.interact(br, ui.scope_id().with("task-next"), egui::Sense::click());
            p.rect_stroke(br, 13.0, egui::Stroke::new(1.0, if b.hovered() { ui::DIM() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
            p.text(br.center(), egui::Align2::CENTER_CENTER, crate::sys::tr("다음 ›", "Next ›"), ui::bold(12.0), if b.hovered() { ui::TEXT() } else { ui::DIM() });
            if b.on_hover_text(crate::sys::tr("이 일은 나중에 하고 다음 일부터 봅니다", "Leave this for later and see the next one")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
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
            ui.painter().text(rest.left_top() + egui::vec2(4.0, 10.0), egui::Align2::LEFT_TOP, crate::sys::tr("답을 반영하는 중입니다. 끝나면 다음 일이 옵니다.", "Applying your answer. The next one comes when it is done."), ui::font(12.5), ui::FAINT());
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50)); // 점이 깜빡인다
    }

    /// 맨 아래의 초기화. 실수로 지우지 않게 두 번 눌러야 한다: 한 번 누르면 확인을 묻고, 6초 안에 다시 눌러야 지운다.
    fn reset_row(&mut self, ui: &mut egui::Ui) {
        let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 20.0), egui::Sense::hover());
        // 왼쪽 끝: 밝은 화면 / 어두운 화면
        let t = Rect::from_min_size(r.rect.left_top(), egui::vec2(60.0, 20.0));
        let tr = ui.interact(t, ui.scope_id().with("theme"), egui::Sense::click());
        let label = if ui::is_light() { crate::sys::tr("어둡게", "Dark") } else { crate::sys::tr("밝게", "Light") };
        p.text(t.left_center(), egui::Align2::LEFT_CENTER, label, ui::font(11.0), if tr.hovered() { ui::TEXT() } else { ui::FAINT() });
        if tr.on_hover_text(crate::sys::tr("밝은 화면과 어두운 화면을 바꿉니다", "Switch between light and dark")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
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
        // 그 옆: 키 (Claude 키를 바꾸거나 Jev 키를 나중에 넣는다)
        let k = Rect::from_min_size(egui::pos2(l.right() + 4.0, r.rect.top()), egui::vec2(54.0, 20.0));
        let kr = ui.interact(k, ui.scope_id().with("keys"), egui::Sense::click());
        p.text(k.left_center(), egui::Align2::LEFT_CENTER, crate::sys::tr("키", "Keys"), ui::font(11.0), if kr.hovered() { ui::TEXT() } else { ui::FAINT() });
        if kr.on_hover_text(crate::sys::tr("API 키를 넣거나 바꿉니다", "Enter or change API keys")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.key_popup = true;
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
            warn.text(r.rect.left_center(), egui::Align2::LEFT_CENTER, crate::sys::tr("화면·대화·기억·기록을 전부 지웁니다", "Erases the board, chat, memory and records"), ui::bold(11.5), ui::RED());
            let y = ui.interact(yes, ui.scope_id().with("reset-yes"), egui::Sense::click());
            let n = ui.interact(no, ui.scope_id().with("reset-no"), egui::Sense::click());
            p.rect_stroke(yes, 10.0, egui::Stroke::new(1.0, ui::RED()), egui::StrokeKind::Inside);
            p.text(yes.center(), egui::Align2::CENTER_CENTER, crate::sys::tr("지우기", "Erase"), ui::bold(11.5), ui::RED());
            p.text(no.center(), egui::Align2::CENTER_CENTER, crate::sys::tr("취소", "Cancel"), ui::bold(11.5), if n.hovered() { ui::TEXT() } else { ui::DIM() });
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
            p.text(b.right_center(), egui::Align2::RIGHT_CENTER, crate::sys::tr("초기화", "Reset"), ui::font(11.0), if resp.hovered() { ui::TEXT() } else { ui::FAINT() });
            if resp.on_hover_text(crate::sys::tr("화면, 대화, 기억, 기록을 전부 지우고 처음부터 시작합니다 (API 키와 설정은 남습니다)", "Erase the board, chat, memory and records and start over (API keys and settings stay)")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
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
        let label = if self.show_shelf { crate::sys::tr("← 대화로 돌아가기", "← Back to chat").to_string() } else { if crate::sys::english() { format!("{n} shelved  ›") } else { format!("보관된 주제 {n}개  ›") } };
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
                ui.label(egui::RichText::new(crate::sys::tr("보관된 주제 · 누르면 화면으로 다시 꺼냅니다", "Shelved topics · click one to bring it back")).size(12.0).color(ui::FAINT()));
                ui.add_space(8.0);
                for s in self.board.shelf.iter().rev() {
                    let (r, p) = ui.allocate_painter(egui::vec2(width - 6.0, 46.0), egui::Sense::click());
                    p.rect_filled(r.rect, 14.0, FIELD());
                    p.rect_stroke(r.rect, 14.0, egui::Stroke::new(1.0, if r.hovered() { ui::DIM() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
                    // 이름이 길어도 줄 밖으로 나가지 않게 이 줄 안에서만 그린다.
                    let inner = p.with_clip_rect(r.rect.shrink2(egui::vec2(12.0, 0.0)));
                    inner.text(r.rect.left_top() + egui::vec2(14.0, 8.0), egui::Align2::LEFT_TOP, &s.topic.name, ui::bold(13.5), ui::TEXT());
                    let titles: Vec<&str> = s.cards.iter().take(4).map(|c| c.title.as_str()).collect();
                    inner.text(r.rect.left_top() + egui::vec2(14.0, 27.0), egui::Align2::LEFT_TOP, if crate::sys::english() { format!("{} cards · {}", s.cards.len(), titles.join(", ")) } else { format!("카드 {}장 · {}", s.cards.len(), titles.join(", ")) }, ui::font(11.0), ui::FAINT());
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
                        ui.label(egui::RichText::new(crate::sys::tr("무엇이든 말하세요.\n답은 오른쪽 화면에 카드로 나타납니다.", "Say anything.\nThe answer appears as cards on the right.")).size(13.5).color(ui::FAINT()));
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
                        .or_else(|| (!self.activity.is_empty() && !self.tidying).then(|| crate::sys::tr("확인하고 있어요.", "Looking into it.").to_string()));
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
                    ui.label(egui::RichText::new(crate::sys::tr("기다리는 중 · 부르거나 하던 얘기로 돌아오면 다시 들어요", "Waiting · call me or come back to the topic and I will listen")).size(13.0).color(ui::FAINT()));
                    ui.add_space(3.0);
                }
                for w in &waiting {
                    ui.label(egui::RichText::new(if crate::sys::english() { format!("Queued · {w}") } else { format!("곧 전함 · {w}") }).size(13.0).color(ui::FAINT()));
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
            let text = if self.heard { crate::sys::tr("듣는 중 · 말이 끝나면 알아서 보냅니다", "Listening · I send it when you stop") } else { crate::sys::tr("듣는 중 · 말씀하세요", "Listening · go ahead") };
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
                crate::sys::tr("말하는 중", "Speaking").to_string()
            } else if writing_down {
                crate::sys::tr("받아쓰는 중", "Transcribing").to_string()
            } else {
                crate::sys::tr("받았어요", "Got it").to_string()
            }
        } else if !self.activity.is_empty() {
            plain(&self.activity)
        } else if !self.got_output {
            match self.pre.as_ref().and_then(|p| p.kind.clone()) {
                Some(_) if crate::sys::english() => "Thinking".to_string(),
                Some(kind) => format!("{kind} · 생각하는 중"),
                None if self.tidying => crate::sys::tr("분류와 주제 정리 중", "Tidying topics").to_string(),
                None => crate::sys::tr("생각하는 중", "Thinking").to_string(),
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
                    if crate::sys::english() {
                        if n > 0 { format!("Writing '{title}' · {n} lines") } else { format!("Writing '{title}'") }
                    } else if n > 0 {
                        format!("'{title}' 쓰는 중 · {n}줄")
                    } else {
                        format!("'{title}' 쓰는 중")
                    }
                }
                None => crate::sys::tr("화면에 쓰는 중", "Writing to the board").to_string(),
            }
        };
        let text = if self.busy { format!("{what} · {}{}", self.sent_at.elapsed().as_secs(), crate::sys::tr("초", "s")) } else { what };
        // 멈춤: 도는 턴을 그만두게 한다. 글자는 이 버튼 왼쪽까지만 그려서 겹치지 않게 한다.
        let mut right = r.rect.right();
        if self.busy {
            let stopping = self.stop.load(std::sync::atomic::Ordering::Relaxed);
            let b = Rect::from_min_size(egui::pos2(r.rect.right() - 50.0, r.rect.top()), egui::vec2(50.0, 22.0));
            right = b.left() - 8.0;
            let resp = ui.interact(b, ui.scope_id().with("stop-turn"), egui::Sense::click());
            p.rect_stroke(b, 11.0, egui::Stroke::new(1.0, if resp.hovered() { ui::TEXT() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
            p.text(b.center(), egui::Align2::CENTER_CENTER, if stopping { crate::sys::tr("멈추는 중", "Stopping") } else { crate::sys::tr("멈춤", "Stop") }, ui::bold(if stopping { 10.5 } else { 11.5 }), if resp.hovered() { ui::TEXT() } else { ui::DIM() });
            if resp.on_hover_text(crate::sys::tr("지금 하는 일을 그만두게 합니다", "Stop what it is doing now")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
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
                    p.text(r.rect.center(), egui::Align2::CENTER_CENTER, crate::sys::tr("빼기", "Remove"), ui::bold(12.0), ui::TEXT());
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
                        .hint_text(if self.busy { crate::sys::tr("답하는 중에도 적으면 이어서 전합니다", "Keep typing, it will be passed along") } else { crate::sys::tr("무엇이든 말하세요 · 사진은 붙여넣거나 끌어다 놓으세요", "Say anything · paste or drop pictures here") })
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
            pp.text(pr.rect.center(), egui::Align2::CENTER_CENTER, crate::sys::tr("사진", "Photo"), ui::bold(13.0), if pr.hovered() { ui::TEXT() } else { ui::DIM() });
            if pr.on_hover_text(crate::sys::tr("사진 파일을 골라 붙입니다 · 캡처한 그림은 Ctrl+V 로 바로 붙습니다", "Attach picture files · paste a capture with Ctrl+V")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                let picked = rfd::FileDialog::new().add_filter(crate::sys::tr("사진", "Pictures"), &["png", "jpg", "jpeg", "webp", "gif", "bmp"]).pick_files().unwrap_or_default();
                for path in picked {
                    self.add_pic(ctx, crate::pics::from_file(&path.display().to_string()));
                }
            }
            let (nr, np) = ui.allocate_painter(egui::vec2(64.0, 40.0), egui::Sense::click());
            np.rect_filled(nr.rect, 20.0, FIELD());
            np.rect_stroke(nr.rect, 20.0, egui::Stroke::new(1.0, if nr.hovered() { ui::DIM() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
            np.text(nr.rect.center(), egui::Align2::CENTER_CENTER, crate::sys::tr("기록", "Log"), ui::bold(13.0), if nr.hovered() { ui::TEXT() } else { ui::DIM() });
            if nr.on_hover_text(crate::sys::tr("오늘의 기록과 기억 보관함 열기", "Open today's record and the memory vault")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                let (date, _) = sys::now_local();
                let today = vault::root().join("기록").join(format!("{date}.md"));
                sys::open_note(&vault::root(), &today);
            }
            // 음성: AI 의 답을 소리로 듣는다.
            let hint = if self.voice { crate::sys::tr("음성 켜짐 · 누르면 끕니다", "Voice on · click to turn off") } else { crate::sys::tr("음성 꺼짐 · 누르면 켭니다", "Voice off · click to turn on") };
            if switch(ui, ctx, "voice-toggle", crate::sys::tr("음성", "Voice"), self.voice, hint) {
                toggle = Some(!self.voice);
            }
            // 대화: 마이크로 말해서 시킨다. 말이 끝나면 알아서 보내고, 답을 다 들으면 다시 듣는다.
            let hint = if crate::eleven::key().is_none() {
                crate::sys::tr("말로 대화하려면 ElevenLabs 키가 필요합니다 (프로그램 폴더의 eleven.txt)", "Talking needs an ElevenLabs key (eleven.txt in the program folder)")
            } else if self.converse {
                crate::sys::tr("말로 대화하는 중 · 누르면 마이크를 끕니다", "Talking · click to turn the microphone off")
            } else {
                crate::sys::tr("누르면 마이크를 켜고 말로 대화합니다", "Click to turn the microphone on and talk")
            };
            if switch(ui, ctx, "mic-toggle", crate::sys::tr("대화", "Talk"), self.converse, hint) && crate::eleven::key().is_some() {
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

    /// 키가 없을 때 입력창 위에 놓는 줄: 지금은 데모라는 것을 알리고, 키를 넣는 창을 다시 열 수 있게 한다.
    fn demo_row(&mut self, ui: &mut egui::Ui) {
        let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());
        p.rect_filled(r.rect, 12.0, ui::ORANGE().gamma_multiply(0.12));
        let tag = Rect::from_min_size(r.rect.min + egui::vec2(8.0, 6.0), egui::vec2(48.0, 18.0));
        p.rect_filled(tag, 9.0, ui::ORANGE());
        p.text(tag.center(), egui::Align2::CENTER_CENTER, "DEMO", ui::bold(10.5), PANEL());
        let button = Rect::from_min_size(egui::pos2(r.rect.right() - 112.0, r.rect.top() + 5.0), egui::vec2(106.0, 20.0));
        let words = p.with_clip_rect(Rect::from_min_max(r.rect.min, egui::pos2(button.left() - 6.0, r.rect.bottom())));
        words.text(
            egui::pos2(tag.right() + 8.0, r.rect.center().y),
            egui::Align2::LEFT_CENTER,
            crate::sys::tr("API 키 없음 · 준비된 장면만 재생", "No API key · prepared scenes only"),
            ui::font(11.5),
            ui::TEXT(),
        );
        let br = ui.interact(button, ui.scope_id().with("open-key"), egui::Sense::click());
        p.rect_stroke(button, 10.0, egui::Stroke::new(1.0, if br.hovered() { ui::TEXT() } else { ui::DIM() }), egui::StrokeKind::Inside);
        p.text(button.center(), egui::Align2::CENTER_CENTER, crate::sys::tr("API 키 넣기", "Enter API key"), ui::bold(11.0), ui::TEXT());
        if br.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.key_popup = true;
        }
        ui.add_space(6.0);
    }

    /// 처음 켰을 때(또는 "API 키 넣기"를 눌렀을 때) 뜨는 창.
    /// 키를 넣으면 프로그램 폴더의 key.txt 에 저장하고 바로 시작한다. 키 없이 데모만 둘러볼 수도 있다.
    pub fn key_window(&mut self, ctx: &egui::Context) {
        if !self.key_popup {
            return;
        }
        let frame = egui::Frame::new().fill(PANEL()).stroke(egui::Stroke::new(1.0, FIELD_EDGE())).corner_radius(24).inner_margin(egui::Margin::same(28));
        let (mut save, mut close) = (false, false);
        egui::Modal::new(egui::Id::unique("key-window")).frame(frame).backdrop_color(egui::Color32::from_black_alpha(170)).show(ctx, |ui| {
            ui.set_width(420.0);
            let has_key = self.cfg.key.is_some();
            let has_jev = crate::jev::key().is_some();
            ui.label(egui::RichText::new(if !self.cfg.demo() { crate::sys::tr("엔진과 키", "Engine and keys") } else { crate::sys::tr("ALI에 오신 것을 환영합니다", "Welcome to ALI") }).font(ui::bold(21.0)).color(ui::TEXT()));
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(crate::sys::tr(
                    "ALI는 Claude로 움직입니다. 본인의 Claude API 키를 쓰거나, 이 컴퓨터에 깔린 Claude Code에 맡길 수 있습니다.\n키는 이 프로그램 폴더의 key.txt 에만 저장되고, Anthropic 말고는 어디로도 보내지지 않습니다.",
                    "ALI runs on Claude. Use your own Claude API key, or the Claude Code installed on this computer.\nThe key is saved only to key.txt in this program's folder and is sent nowhere except Anthropic.",
                ))
                .size(12.5)
                .color(ui::DIM()),
            );
            ui.add_space(14.0);
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
            // 엔진: Claude API 를 직접 부를지, 이 컴퓨터의 Claude Code 에 맡길지.
            let cc_version = crate::cc::version();
            let pick_cc = *self.engine_pick.get_or_insert(self.cfg.cc);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let half = (ui.available_width() - 8.0) / 2.0;
                let choice = |ui: &mut egui::Ui, on: bool, title: &str, sub: &str, enabled: bool| -> bool {
                    let (r, p) = ui.allocate_painter(egui::vec2(half, 52.0), if enabled { egui::Sense::click() } else { egui::Sense::hover() });
                    p.rect_filled(r.rect, 14.0, if on { ui::ORANGE().gamma_multiply(0.14) } else { FIELD() });
                    p.rect_stroke(r.rect, 14.0, egui::Stroke::new(if on { 1.5 } else { 1.0 }, if on { ui::ORANGE() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
                    p.text(r.rect.left_top() + egui::vec2(14.0, 17.0), egui::Align2::LEFT_CENTER, title, ui::bold(13.0), if enabled { ui::TEXT() } else { ui::FAINT() });
                    p.text(r.rect.left_top() + egui::vec2(14.0, 36.0), egui::Align2::LEFT_CENTER, sub, ui::font(11.0), ui::FAINT());
                    enabled && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                };
                if choice(ui, !pick_cc, crate::sys::tr("Claude API 키", "Claude API key"), crate::sys::tr("ALI가 직접 호출 · 쓴 만큼 과금", "ALI calls the API · pay per use"), true) {
                    self.engine_pick = Some(false);
                }
                let sub = match &cc_version {
                    Some(v) => format!("{} {v}", crate::sys::tr("설치됨", "installed")),
                    None => crate::sys::tr("이 컴퓨터에 없음", "not found on this computer").to_string(),
                };
                if choice(ui, pick_cc, "Claude Code", &sub, cc_version.is_some()) {
                    self.engine_pick = Some(true);
                }
            });
            let pick_cc = self.engine_pick.unwrap_or(false) && cc_version.is_some();
            ui.add_space(12.0);
            if pick_cc {
                ui.label(
                    egui::RichText::new(crate::sys::tr(
                        "이 컴퓨터의 Claude Code에 맡깁니다. Claude Code의 로그인, CLAUDE.md, 스킬, 훅, MCP 서버, 권한 설정이 그대로 적용됩니다. API 키는 필요 없습니다.
파일·명령·웹 작업은 Claude Code의 도구로 하고, 경로 지도와 기억 노트 같은 ALI 도구는 그대로 씁니다. 작업자와 기억 자동 정리는 API 키가 있을 때만 돕니다.",
                        "ALI hands each turn to the Claude Code on this computer. Its login, CLAUDE.md, skills, hooks, MCP servers and permission settings all apply. No API key is needed.
Files, commands and the web go through Claude Code's tools; ALI's own tools such as route maps and memory notes keep working. Workers and automatic memory keeping still need an API key.",
                    ))
                    .size(12.0)
                    .color(ui::DIM()),
                );
                ui.add_space(12.0);
                // 권한: Claude Code 가 파일을 고치거나 명령을 실행할 때 어디까지 묻지 않고 하게 할지.
                ui.label(egui::RichText::new(crate::sys::tr("묻지 않고 해도 되는 일", "What it may do without asking")).font(ui::bold(12.5)).color(ui::TEXT()));
                ui.add_space(4.0);
                let now = self.perm_pick.get_or_insert_with(crate::cc::permission).clone();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let third = (ui.available_width() - 12.0) / 3.0;
                    for (value, name) in [
                        ("default", crate::sys::tr("내 설정대로", "My settings")),
                        ("acceptEdits", crate::sys::tr("파일 수정", "File edits")),
                        ("bypassPermissions", crate::sys::tr("전부", "Everything")),
                    ] {
                        let on = now == value;
                        let (r, p) = ui.allocate_painter(egui::vec2(third, 30.0), egui::Sense::click());
                        p.rect_filled(r.rect, 10.0, if on { ui::ORANGE().gamma_multiply(0.14) } else { FIELD() });
                        p.rect_stroke(r.rect, 10.0, egui::Stroke::new(1.0, if on { ui::ORANGE() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
                        p.text(r.rect.center(), egui::Align2::CENTER_CENTER, name, ui::bold(12.0), ui::TEXT());
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.perm_pick = Some(value.to_string());
                        }
                    }
                });
                ui.add_space(5.0);
                ui.label(
                    egui::RichText::new(match now.as_str() {
                        "acceptEdits" => crate::sys::tr("파일을 만들고 고치는 것은 묻지 않고 합니다. 그 밖의 명령은 Claude Code 설정을 따릅니다.", "Creates and edits files without asking. Other commands follow your Claude Code settings."),
                        "bypassPermissions" => crate::sys::tr("명령 실행과 파일 수정을 전부 묻지 않고 합니다. API 키 방식의 ALI와 같습니다.", "Runs commands and edits files without asking, as ALI does with an API key."),
                        _ => crate::sys::tr("Claude Code에서 허용해 둔 것만 합니다. 나머지는 거절되고, 그렇다고 알려 줍니다.", "Does only what your Claude Code settings allow. The rest is refused, and it tells you so."),
                    })
                    .size(11.5)
                    .color(ui::FAINT()),
                );
                ui.add_space(14.0);
            }
            let caption = |ui: &mut egui::Ui, name: &str, state: &str, good: bool| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(name).font(ui::bold(12.5)).color(ui::TEXT()));
                    ui.label(egui::RichText::new(state).size(11.5).color(if good { ui::GREEN() } else { ui::FAINT() }));
                });
                ui.add_space(4.0);
            };
            if !pick_cc {
            caption(ui, crate::sys::tr("Claude API 키", "Claude API key"), if has_key { crate::sys::tr("저장됨", "saved") } else { crate::sys::tr("필수", "required") }, has_key);
            field(ui, &mut self.key_input, if has_key { crate::sys::tr("비워 두면 지금 키를 그대로 씁니다", "Leave empty to keep the current key") } else { "sk-ant-…" });
            ui.add_space(6.0);
            let link = ui.add(egui::Label::new(egui::RichText::new(crate::sys::tr("키가 없으신가요? Claude Console에서 만들기 ↗", "No key yet? Create one in the Claude Console ↗")).size(12.0).color(ui::ORANGE())).sense(egui::Sense::click()));
            if link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                ctx.open_url(egui::OpenUrl::new_tab("https://console.anthropic.com/settings/keys"));
            }
            ui.add_space(14.0);
            }
            caption(ui, crate::sys::tr("TypeSafe Jev 키", "TypeSafe Jev key"), if has_jev { crate::sys::tr("저장됨", "saved") } else { crate::sys::tr("선택", "optional") }, has_jev);
            field(ui, &mut self.jev_input, if has_jev { crate::sys::tr("비워 두면 지금 키를 그대로 씁니다", "Leave empty to keep the current key") } else { crate::sys::tr("없어도 됩니다", "Not needed to start") });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(crate::sys::tr(
                    "Jev는 글을 쓰지 않고 판단만 하는 작고 빠른 모델입니다. 넣으면 말이 어느 주제에 속하는지 미리 고르고, 중요한 카드를 앞에 놓고, 겹치는 주제를 정리하고, 옛 대화에서 필요한 것만 골라 옵니다. 없어도 ALI는 그대로 동작합니다.",
                    "Jev is a small, fast model that only makes decisions. With it, ALI picks the topic before answering, puts important cards first, tidies overlapping topics, and recalls only the past conversations that matter. Without it, ALI works the same.",
                ))
                .size(11.5)
                .color(ui::FAINT()),
            );
            ui.add_space(16.0);
            let ready = if pick_cc { pick_cc != self.cfg.cc || self.jev_input.trim().len() > 8 || self.perm_pick.as_deref().map(|p| p != crate::cc::permission()).unwrap_or(false) } else { self.key_input.trim().len() > 20 || (has_key && (self.jev_input.trim().len() > 8 || self.cfg.cc)) };
            let has_key = has_key || self.cfg.cc;
            let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 42.0), egui::Sense::click());
            p.rect_filled(r.rect, 21.0, if ready { ui::TEXT() } else { FIELD() });
            p.text(r.rect.center(), egui::Align2::CENTER_CENTER, if has_key { crate::sys::tr("저장", "Save") } else { crate::sys::tr("저장하고 시작", "Save and start") }, ui::bold(13.5), if ready { PANEL() } else { ui::FAINT() });
            if ready && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                save = true;
            }
            ui.add_space(8.0);
            let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 38.0), egui::Sense::click());
            p.rect_stroke(r.rect, 19.0, egui::Stroke::new(1.0, if r.hovered() { ui::DIM() } else { FIELD_EDGE() }), egui::StrokeKind::Inside);
            p.text(r.rect.center(), egui::Align2::CENTER_CENTER, if has_key { crate::sys::tr("닫기", "Close") } else { crate::sys::tr("키 없이 데모 둘러보기", "Look around the demo without a key") }, ui::bold(13.0), ui::TEXT());
            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                close = true;
            }
            if !has_key {
                ui.add_space(10.0);
                ui.label(egui::RichText::new(crate::sys::tr("데모에서는 무엇을 입력해도 준비된 장면이 재생됩니다. AI가 실제로 답하지는 않습니다.", "In the demo, anything you type plays prepared scenes. The AI does not actually answer.")).size(11.5).color(ui::FAINT()));
            }
        });
        if save {
            let base = sys::base_dir();
            let was_demo = self.cfg.demo();
            let _ = std::fs::write(base.join("engine.txt"), if self.engine_pick == Some(true) && crate::cc::version().is_some() { "claude-code" } else { "api" });
            if let Some(p) = self.perm_pick.take() {
                let _ = std::fs::write(base.join("claude-permission.txt"), p);
            }
            if !self.key_input.trim().is_empty() {
                let _ = std::fs::write(base.join("key.txt"), self.key_input.trim());
            }
            if !self.jev_input.trim().is_empty() {
                let _ = std::fs::write(base.join("jev.txt"), self.jev_input.trim());
            }
            self.key_input.clear();
            self.jev_input.clear();
            self.cfg = llm::Config::load();
            // 데모에서 넘어올 때만 화면을 비우고 실제 모드로 들어간다. 키만 바꾼 경우에는 하던 일을 그대로 둔다.
            if was_demo && !self.cfg.demo() {
                self.recent.clear();
                self.view = ui::View::default();
                self.start_real_mode();
            }
            self.key_popup = false;
            self.engine_pick = None;
        }
        if close {
            self.key_popup = false;
            self.engine_pick = None;
            self.perm_pick = None;
        }
    }
}
