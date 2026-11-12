//! 왼쪽에 고정된 대화 구역: 주고받은 말, 진행 표시, 입력창, 버튼.

use eframe::egui;

use crate::{App, llm, sys, ui, vault};

const PANEL: egui::Color32 = egui::Color32::from_rgb(21, 22, 25);
const PANEL_EDGE: egui::Color32 = egui::Color32::from_rgb(33, 34, 38);
const FIELD: egui::Color32 = egui::Color32::from_rgb(28, 29, 32);
const FIELD_EDGE: egui::Color32 = egui::Color32::from_rgb(50, 51, 56);
pub const WIDTH: f32 = 380.0;

/// 이름이 붙은 스위치. 손잡이가 오른쪽에 밝게 있으면 켜진 것, 왼쪽에 어둡게 있으면 꺼진 것. 눌렸으면 true.
fn switch(ui: &mut egui::Ui, ctx: &egui::Context, id: &str, label: &str, on: bool, hint: &str) -> bool {
    let (r, p) = ui.allocate_painter(egui::vec2(104.0, 40.0), egui::Sense::click());
    let k = ui::ease_in_out_cubic(ctx.animate_bool_with_time(ui.scope_id().with(id), on, 0.28));
    let lerp = |a: egui::Color32, b: egui::Color32| {
        let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k) as u8;
        egui::Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
    };
    p.rect_filled(r.rect, 20.0, FIELD);
    p.rect_stroke(r.rect, 20.0, egui::Stroke::new(1.0, if r.hovered() { ui::DIM } else { FIELD_EDGE }), egui::StrokeKind::Inside);
    p.text(r.rect.left_center() + egui::vec2(18.0, 0.0), egui::Align2::LEFT_CENTER, label, ui::bold(13.0), lerp(ui::DIM, ui::TEXT));
    let track = egui::Rect::from_center_size(r.rect.right_center() - egui::vec2(32.0, 0.0), egui::vec2(36.0, 20.0));
    p.rect_filled(track, 10.0, lerp(egui::Color32::from_rgb(52, 53, 58), ui::TEXT));
    p.circle_filled(egui::pos2(track.left() + 10.0 + 16.0 * k, track.center().y), 7.0, lerp(ui::DIM, FIELD));
    r.on_hover_text(hint).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

impl App {
    pub fn chat_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, talking: bool) {
        egui::Panel::left("chat")
            .exact_size(WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(ui::BG).inner_margin(egui::Margin { left: 18, right: 0, top: 18, bottom: 18 }))
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(egui::Stroke::new(1.0, PANEL_EDGE))
                    .corner_radius(26)
                    .inner_margin(egui::Margin::same(18))
                    .show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        // 아래에서 위로 쌓는다: 맨 아래에 상태 줄, 그 위에 버튼, 입력창, 진행 표시. 남는 위쪽이 대화.
                        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                            ui.label(egui::RichText::new(self.status_line()).size(10.5).color(ui::FAINT));
                            ui.add_space(8.0);
                            self.buttons(ui, ctx);
                            ui.add_space(8.0);
                            if self.cfg.key.is_none() && std::env::var("LYBOARD_DEMO").is_err() {
                                self.key_setup(ui);
                            } else {
                                self.input_box(ui, ctx);
                            }
                            ui.add_space(8.0);
                            if let Some(e) = self.error.clone() {
                                ui.label(egui::RichText::new(e).size(12.5).color(ui::RED));
                                ui.add_space(4.0);
                            }
                            self.progress(ui, ctx, talking);
                            ui.add_space(6.0);
                            self.history(ui);
                        });
                    });
            });
    }

    /// 주고받은 말. 화면에 글을 쌓지 않는 것이 이 프로그램의 원칙이므로, AI 쪽은 한마디(say)만 남는다.
    fn history(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).max_height(ui.available_height()).show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(width);
                let mine = |ui: &mut egui::Ui, text: &str| {
                    ui.label(egui::RichText::new(text).size(13.0).color(ui::DIM));
                };
                let yours = |ui: &mut egui::Ui, text: &str, strong: bool| {
                    let t = egui::RichText::new(text).font(ui::bold(15.0)).color(if strong { ui::TEXT } else { ui::DIM });
                    ui.label(t);
                    ui.add_space(14.0);
                };
                let n = self.recent.len();
                if n == 0 && !self.busy {
                    if self.board.say.is_empty() {
                        ui.label(egui::RichText::new("무엇이든 말하세요.\n답은 오른쪽 화면에 카드로 나타납니다.").size(13.5).color(ui::FAINT));
                    } else {
                        yours(ui, &self.board.say.clone(), true);
                    }
                }
                for (i, (u, a)) in self.recent.iter().enumerate() {
                    mine(ui, u);
                    ui.add_space(3.0);
                    yours(ui, a, i + 1 == n && !self.busy);
                }
                if self.busy {
                    mine(ui, &self.cur_input.clone());
                    ui.add_space(3.0);
                    // 지금 오고 있는 답: 말하는 중이면 그 문장, 아니면 흘러 들어오는 한마디
                    let live = self.caption.clone().or_else(|| self.say_live.clone()).or_else(|| self.got_output.then(|| self.board.say.clone()));
                    if let Some(text) = live.filter(|t| !t.is_empty()) {
                        yours(ui, &text, true);
                    }
                } else if let Some(c) = self.caption.clone() {
                    yours(ui, &c, true);
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
            p.circle_stroke(dot, 4.0 + size, egui::Stroke::new(1.3, ui::TEXT.gamma_multiply(0.6)));
            p.circle_filled(dot, 3.0, ui::TEXT);
            let text = if self.heard { "듣는 중 · 말이 끝나면 알아서 보냅니다" } else { "듣는 중 · 말씀하세요" };
            p.with_clip_rect(r.rect).text(r.rect.left_center() + egui::vec2(24.0, 0.0), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(12.5), ui::TEXT);
            ctx.request_repaint();
            return;
        }
        if talking {
            // 말하는 중: 목소리처럼 일렁이는 고리
            for (i, speed) in [5.3_f64, 7.9, 3.7].iter().enumerate() {
                let wave = ((now * speed + i as f64 * 1.3).sin() * 0.5 + 0.5) as f32;
                p.circle_stroke(dot, 3.5 + i as f32 * 2.0 + wave * 2.0, egui::Stroke::new(1.1, ui::TEXT.gamma_multiply(0.75 - i as f32 * 0.22)));
            }
            p.circle_filled(dot, 3.0, ui::TEXT);
        } else {
            let k = ui::ease_out_cubic((now * 0.9).fract() as f32);
            p.circle_stroke(dot, 3.0 + 6.0 * k, egui::Stroke::new(1.2, ui::ORANGE.gamma_multiply(0.6 * (1.0 - k))));
            p.circle_filled(dot, 3.0, ui::ORANGE);
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
            self.activity.clone()
        } else if !self.got_output {
            match self.pre.as_ref().and_then(|p| p.kind.clone()) {
                Some(kind) => format!("{kind} · 생각하는 중"),
                None => "생각하는 중".to_string(),
            }
        } else {
            "화면에 쓰는 중".to_string()
        };
        let text = if self.busy { format!("{what} · {}초", self.sent_at.elapsed().as_secs()) } else { what };
        let clip = p.with_clip_rect(r.rect);
        clip.text(r.rect.left_center() + egui::vec2(24.0, 0.0), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(12.5), ui::ORANGE);
        ctx.request_repaint();
    }

    fn input_box(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let now = ui.input(|i| i.time);
        let pill = egui::Frame::new()
            .fill(FIELD)
            .stroke(egui::Stroke::new(1.0, FIELD_EDGE))
            .corner_radius(20)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.input)
                        .hint_text(if self.busy { "답하는 중…" } else { "무엇이든 말하세요" })
                        .desired_width(f32::INFINITY)
                        .desired_rows(2)
                        // Enter 는 보내기, 줄을 바꾸려면 Shift+Enter
                        .return_key(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter))
                        .font(egui::FontId::proportional(15.0))
                        .text_color(ui::TEXT)
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
            let (nr, np) = ui.allocate_painter(egui::vec2(76.0, 40.0), egui::Sense::click());
            np.rect_filled(nr.rect, 20.0, FIELD);
            np.rect_stroke(nr.rect, 20.0, egui::Stroke::new(1.0, if nr.hovered() { ui::DIM } else { FIELD_EDGE }), egui::StrokeKind::Inside);
            np.text(nr.rect.center(), egui::Align2::CENTER_CENTER, "기록", ui::bold(13.0), if nr.hovered() { ui::TEXT } else { ui::DIM });
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
            egui::Frame::new().fill(FIELD).stroke(egui::Stroke::new(1.0, FIELD_EDGE)).corner_radius(14).inner_margin(egui::Margin::symmetric(14, 9)).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(value)
                        .password(true)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY)
                        .font(egui::FontId::proportional(13.5))
                        .text_color(ui::TEXT)
                        .frame(egui::Frame::NONE),
                )
            });
        };
        // 아래에서 위로 쌓이므로 버튼이 먼저 온다.
        let ready = self.key_input.trim().len() > 20;
        let (r, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
        p.rect_filled(r.rect, 20.0, if ready { ui::TEXT } else { FIELD });
        p.text(r.rect.center(), egui::Align2::CENTER_CENTER, "저장하고 시작", ui::bold(13.5), if ready { PANEL } else { ui::FAINT });
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
                .color(ui::DIM),
        );
    }
}
