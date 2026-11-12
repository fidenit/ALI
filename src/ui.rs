//! 보드를 그린다. 배치 계산, 전환, 부품별 그리기, 사용자의 직접 조작.
//! 사용자의 조작은 화면 수정 언어 한 줄로 바꿔서 돌려준다 (AI 의 조작과 같은 경로를 탄다).
//!
//! 디자인: 무광 검정. 채도 없는 회색조, 큰 둥근 모서리, 굵은 제목과 숫자,
//! "굵은 항목명 + 옅은 값", 테두리만 있는 알약형 버튼.
//! 모든 변화는 시간 기반 이징(ease in/out)으로 움직인다.

use std::collections::{HashMap, HashSet};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontFamily, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind,
    Vec2, pos2, vec2,
};

use crate::core::{Board, Card, Kind};

pub const CW: f32 = 120.0;
pub const CH: f32 = 104.0;
pub const GAP: f32 = 14.0;
const HEAD: f32 = 46.0;
const PAD: f32 = 22.0;
const RADIUS: u8 = 20;

pub const BG: Color32 = Color32::from_rgb(16, 17, 19);
const CARD: Color32 = Color32::from_rgb(28, 29, 32); // #1C1D20
const RAISED: Color32 = Color32::from_rgb(41, 42, 46);
const BORDER: Color32 = Color32::from_rgb(50, 51, 56);
const EDGE: Color32 = Color32::from_rgb(104, 105, 111);
pub const TEXT: Color32 = Color32::from_rgb(218, 219, 223);
const SOFT: Color32 = Color32::from_rgb(184, 185, 190);
pub const DIM: Color32 = Color32::from_rgb(138, 139, 145);
pub const FAINT: Color32 = Color32::from_rgb(88, 89, 95);
const GREEN: Color32 = Color32::from_rgb(142, 198, 162);
const AMBER: Color32 = Color32::from_rgb(216, 186, 130);
pub const RED: Color32 = Color32::from_rgb(222, 134, 128);

// 전환 시간 (초)
const T_MOVE: f64 = 0.42;
const T_IN: f64 = 0.38;
const T_OUT: f64 = 0.26;
const T_ITEM: f64 = 0.34;
const T_NUM: f64 = 0.65;
const T_FLASH: f64 = 1.1;
/// 어느 분류에도 해당하지 않는 이름 (분류 화면에서 주제를 하나도 놓지 않을 때 쓴다)
const NO_GROUP: &str = "{1}none";

// ---- 이징 함수 (easings.net 의 표준 곡선) ----
pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}
pub fn ease_in_cubic(t: f32) -> f32 {
    t * t * t
}
pub fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}
fn unit(now: f64, t0: f64, dur: f64) -> f32 {
    (((now - t0) / dur) as f32).clamp(0.0, 1.0)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

fn font(size: f32) -> FontId {
    FontId::proportional(size)
}
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

/// 출발값에서 목표값으로 이징하며 가는 2차원 값.
struct Tween {
    from: Vec2,
    to: Vec2,
    t0: f64,
}

impl Tween {
    fn fixed(v: Vec2) -> Self {
        Tween { from: v, to: v, t0: 0.0 }
    }
    fn at(&self, now: f64) -> Vec2 {
        self.from + (self.to - self.from) * ease_in_out_cubic(unit(now, self.t0, T_MOVE))
    }
    fn retarget(&mut self, to: Vec2, now: f64) {
        if (to - self.to).length() > 0.5 {
            self.from = self.at(now);
            self.to = to;
            self.t0 = now;
        }
    }
    fn busy(&self, now: f64) -> bool {
        now - self.t0 < T_MOVE && self.from != self.to
    }
}

struct CardView {
    pos: Tween,
    size: Tween,
    born: f64,
    rev: u64,
    flash: f64,
    dragging: bool,
    resizing: bool,
    /// 마지막으로 그린 내용. 카드가 지워지면 이것으로 사라지는 모습을 그린다.
    snap: Card,
    dying: Option<f64>,
    /// 화면을 내려도 제자리에 붙어 있는 카드인가 ("내 차례" 구역)
    sticky: bool,
    /// 내용이 실제로 차지한 높이 (다음 프레임의 카드 높이 계산에 쓴다)
    content_h: f32,
    /// 항목별 등장 시각
    items: HashMap<String, f64>,
    /// 숫자 값의 전환 (막대 길이, 진행률)
    nums: HashMap<String, (f32, f32, f64)>,
}

#[derive(Default)]
pub struct View {
    cards: HashMap<String, CardView>,
    /// 주제 구역의 위치·크기 전환과 등장 시각
    zones: HashMap<String, (Tween, Tween, f64)>,
    /// 카드별 중요도 (판단 모델이 매긴 것). 구역 안에서 중요한 카드를 앞에 놓는 데 쓴다.
    importance: HashMap<String, f64>,
    /// 판에서 지금 보고 있는 범위 (확대·축소와 이동의 결과)
    scene: Option<Rect>,
    /// 다른 곳으로 날아가는 중: (출발 범위, 도착 범위, 시작 시각)
    fly: Option<(Rect, Rect, f64)>,
    /// 지금의 이동이 "더 축소할 수 없어서 튕겨 돌아오는 것"인가
    bounce: bool,
    /// "내 차례" 구역을 스크롤한 거리
    inbox_scroll: f32,
    /// 지금의 확대 비율 (1 = 제 크기)
    pub zoom: f32,
    /// 다음 프레임에 화면에 들어오게 할 주제
    goto: Option<String>,
}

impl View {
    /// 이 주제가 화면에 보이게 해 달라고 예약한다 (안 보이거나 너무 작으면 그쪽으로 날아간다).
    pub fn want_topic(&mut self, topic: String) {
        self.goto = Some(topic);
    }

    pub fn set_importance(&mut self, m: HashMap<String, f64>) {
        // 이번에 검토한 카드의 값만 고친다 (나머지 카드의 값은 그대로 둔다).
        self.importance.extend(m);
    }
}

/// 웹 카드 하나가 화면에서 차지한 자리.
pub struct WebSlot {
    pub id: String,
    pub rect: Rect,
    pub url: String,
    pub visible: bool,
}

const ZONE: Color32 = Color32::from_rgb(21, 22, 25);
const ZONE_EDGE: Color32 = Color32::from_rgb(33, 34, 38);

/// 카드가 실제로 차지할 세로 칸 수. 내용이 짧으면 줄어든다 (사용자가 크기를 정한 카드는 그대로).
fn eff_h(c: &Card, view: &View) -> i32 {
    let h = c.h as i32;
    if c.pinned {
        return h;
    }
    match view.cards.get(&c.id) {
        Some(v) if v.content_h > 0.0 => {
            let need = ((HEAD + v.content_h + 12.0 + GAP) / (CH + GAP)).ceil() as i32;
            need.clamp(1, h)
        }
        _ => h,
    }
}

// 주제 구역: 안쪽 여백, 제목 줄 높이, 구역 사이 간격
const ZONE_PAD: f32 = 14.0;
const ZONE_HEAD: f32 = 44.0;
const ZONE_GAP: f32 = 16.0;

/// 카드 한 장이 놓일 자리.
struct Slot {
    /// 보드 기준 위치 (픽셀)
    pos: Vec2,
    w: i32,
    h: i32,
    col: i32,
    row: i32,
    /// 화면을 내려도 제자리에 붙어 있는 카드인가 ("내 차례" 구역의 카드)
    sticky: bool,
    /// 이 카드가 속한 주제 구역 안에서 격자가 시작되는 지점과, 그 구역의 가로 칸 수
    origin: Vec2,
    limit: i32,
}

/// 주제 하나의 구역.
struct Zone {
    id: String,
    name: String,
    min: Vec2,
    size: Vec2,
    /// 답이 필요한 카드가 모이는 "내 차례" 구역인가
    inbox: bool,
}

/// 배치. 주제마다 구역을 하나씩 만들고, 카드는 자기 주제의 구역 안에서만 격자에 놓는다.
/// 구역 안에서는 위치가 정해진 카드를 먼저 놓고 나머지를 빈 곳에 채운다.
/// 화면이 넓으면 구역을 두 줄로 나란히 놓아 여러 주제가 한 화면에 보이게 한다.
fn layout(board: &Board, width: f32, view: &View, only: Option<&str>, with_inbox: bool) -> (HashMap<String, Slot>, Vec<Zone>) {
    let cells = |avail: f32| (((avail - 2.0 * ZONE_PAD + GAP) / (CW + GAP)).floor() as i32).max(2);
    // 답이 필요한 카드(decision)는 주제 구역에 두지 않고 오른쪽 "내 차례" 구역으로 모은다.
    let asks: Vec<&Card> = board.cards.iter().filter(|c| c.kind == Kind::Decision).collect();
    let inbox_w = INBOX_COLS as f32 * (CW + GAP) - GAP + 2.0 * ZONE_PAD;
    let full_width = width;
    let width = if asks.is_empty() || !with_inbox { width } else { (width - inbox_w - ZONE_GAP).max(CW * 3.0) };
    let topics: Vec<&crate::core::Topic> =
        board.topics.iter().filter(|t| only.map(|g| t.group.as_deref().unwrap_or("") == g).unwrap_or(true)).filter(|t| board.cards.iter().any(|c| c.topic == t.id && c.kind != Kind::Decision)).collect();
    let half = cells((width - ZONE_GAP) / 2.0);
    let limit = if topics.len() >= 2 && half >= 4 { half } else { cells(width) };

    let mut slots = HashMap::new();
    let mut zones = Vec::new();
    let (mut x, mut y, mut shelf_h) = (0.0_f32, 0.0_f32, 0.0_f32);
    for t in topics {
        let mut used: HashSet<(i32, i32)> = HashSet::new();
        let mut placed: Vec<(&Card, i32, i32, i32, i32)> = Vec::new();
        let width_of = |c: &Card| (c.w as i32).min(limit);
        let occupy = |used: &mut HashSet<(i32, i32)>, col: i32, row: i32, w: i32, h: i32| {
            for cx in col..col + w {
                for cy in row..row + h {
                    used.insert((cx, cy));
                }
            }
        };
        let mine: Vec<&Card> = board.cards.iter().filter(|c| c.topic == t.id && c.kind != Kind::Decision).collect();
        for c in mine.iter().filter(|c| c.grid.is_some()) {
            let (col, row) = c.grid.unwrap();
            let (w, h) = (width_of(c), eff_h(c, view));
            let col = col.min((limit - w).max(0));
            occupy(&mut used, col, row, w, h);
            placed.push((c, col, row, w, h));
        }
        // 자리가 정해지지 않은 카드는 중요한 것부터 놓는다 (세 단계로만 나눠서 작은 차이로 자리가 바뀌지 않게 한다).
        let mut free: Vec<&&Card> = mine.iter().filter(|c| c.grid.is_none()).collect();
        free.sort_by_key(|c| -((view.importance.get(&c.id).copied().unwrap_or(0.5) * 2.99) as i32));
        for c in free {
            let (w, h) = (width_of(c), eff_h(c, view));
            let mut row = 0;
            let (col, row) = 'find: loop {
                for col in 0..=(limit - w) {
                    if (col..col + w).all(|cx| (row..row + h).all(|cy| !used.contains(&(cx, cy)))) {
                        break 'find (col, row);
                    }
                }
                row += 1;
            };
            occupy(&mut used, col, row, w, h);
            placed.push((c, col, row, w, h));
        }
        let ncols = placed.iter().map(|p| p.1 + p.3).max().unwrap_or(1);
        let nrows = placed.iter().map(|p| p.2 + p.4).max().unwrap_or(1);
        let size = vec2(
            ncols as f32 * (CW + GAP) - GAP + 2.0 * ZONE_PAD,
            ZONE_HEAD + nrows as f32 * (CH + GAP) - GAP + ZONE_PAD,
        );
        if x > 0.0 && x + size.x > width + 1.0 {
            x = 0.0;
            y += shelf_h + ZONE_GAP;
            shelf_h = 0.0;
        }
        let min = vec2(x, y);
        let origin = min + vec2(ZONE_PAD, ZONE_HEAD);
        for (c, col, row, w, h) in placed {
            let pos = origin + vec2(col as f32 * (CW + GAP), row as f32 * (CH + GAP));
            slots.insert(c.id.clone(), Slot { pos, w, h, col, row, origin, limit, sticky: false });
        }
        zones.push(Zone { id: t.id.clone(), name: t.name.clone(), min, size, inbox: false });
        x += size.x + ZONE_GAP;
        shelf_h = shelf_h.max(size.y);
    }
    // "내 차례" 구역: 화면 오른쪽 끝에 세로로 쌓는다. 급한 것(나중에 온 것)이 위로 온다.
    if with_inbox && !asks.is_empty() {
        let min = vec2((full_width - inbox_w).max(0.0), 0.0);
        let origin = min + vec2(ZONE_PAD, ZONE_HEAD);
        let mut row = 0;
        for c in asks.iter().rev() {
            let h = eff_h(c, view);
            let pos = origin + vec2(0.0, row as f32 * (CH + GAP));
            slots.insert(c.id.clone(), Slot { pos, w: INBOX_COLS, h, col: 0, row, origin, limit: INBOX_COLS, sticky: true });
            row += h;
        }
        let size = vec2(inbox_w, ZONE_HEAD + row as f32 * (CH + GAP) - GAP + ZONE_PAD);
        zones.push(Zone { id: INBOX_ID.into(), name: "내 차례".into(), min, size, inbox: true });
    }
    (slots, zones)
}

/// "내 차례" 구역의 가로 칸 수와 내부 이름
const INBOX_COLS: i32 = 3;
const INBOX_ID: &str = "\u{1}inbox";

pub const ORANGE: Color32 = Color32::from_rgb(255, 150, 50);

/// 이름 붙은 숫자 하나를 목표값으로 이징한다. 반환값은 (현재 값, 아직 움직이는 중인가).
fn tween_num(map: &mut HashMap<String, (f32, f32, f64)>, key: &str, target: f32, now: f64, dur: f64) -> (f32, bool) {
    let e = map.entry(key.to_string()).or_insert((target, target, now - dur));
    if (e.1 - target).abs() > 1e-4 {
        *e = (e.0 + (e.1 - e.0) * ease_out_cubic(unit(now, e.2, dur)), target, now);
    }
    let t = unit(now, e.2, dur);
    (e.0 + (e.1 - e.0) * ease_out_cubic(t), t < 1.0)
}

/// 둥근 사각형 둘레를 시계 방향으로 d 만큼 간 지점.
fn perimeter_point(r: Rect, rad: f32, d: f32) -> Pos2 {
    use std::f32::consts::{FRAC_PI_2, PI};
    let (w, h) = (r.width() - 2.0 * rad, r.height() - 2.0 * rad);
    let arc = FRAC_PI_2 * rad;
    let mut d = d.rem_euclid(2.0 * (w + h) + 4.0 * arc);
    let on = |center: Pos2, a: f32| center + vec2(a.cos(), a.sin()) * rad;
    if d < w {
        return pos2(r.left() + rad + d, r.top());
    }
    d -= w;
    if d < arc {
        return on(pos2(r.right() - rad, r.top() + rad), -FRAC_PI_2 + d / rad);
    }
    d -= arc;
    if d < h {
        return pos2(r.right(), r.top() + rad + d);
    }
    d -= h;
    if d < arc {
        return on(pos2(r.right() - rad, r.bottom() - rad), d / rad);
    }
    d -= arc;
    if d < w {
        return pos2(r.right() - rad - d, r.bottom());
    }
    d -= w;
    if d < arc {
        return on(pos2(r.left() + rad, r.bottom() - rad), FRAC_PI_2 + d / rad);
    }
    d -= arc;
    if d < h {
        return pos2(r.left(), r.bottom() - rad - d);
    }
    on(pos2(r.left() + rad, r.top() + rad), PI + (d - h) / rad)
}

/// 처리 중 표시: 주황색 선이 꼬리를 끌며 카드 테두리를 돈다. `k` 는 표시 강도(0~1).
pub fn draw_runner(p: &Painter, rect: Rect, rad: f32, now: f64, k: f32) {
    let r = rect.shrink(0.75);
    let rad = rad - 0.75;
    let total = 2.0 * (r.width() + r.height() - 4.0 * rad) + std::f32::consts::TAU * rad;
    p.rect_stroke(rect, rad, Stroke::new(1.0, ORANGE.gamma_multiply(0.22 * k)), StrokeKind::Inside);
    let head = ((now / 1.7).fract() as f32) * total;
    let len = total * 0.3;
    let n = 44;
    for i in 0..n {
        let (a, b) = (i as f32 / n as f32, (i + 1) as f32 / n as f32);
        let alpha = b * b * k;
        p.line_segment(
            [perimeter_point(r, rad, head - len + len * a), perimeter_point(r, rad, head - len + len * b)],
            Stroke::new(2.0, ORANGE.gamma_multiply(alpha)),
        );
    }
    p.circle_filled(perimeter_point(r, rad, head), 2.2, ORANGE.gamma_multiply(k));
}

/// 본문을 그릴 때 필요한 것들.
struct Ctx<'a> {
    ui: &'a egui::Ui,
    now: f64,
    id: egui::Id,
    /// 카드 전체의 불투명도 (등장·퇴장 중이면 1보다 작다)
    alpha: f32,
    /// 사라지는 중인 카드는 조작을 받지 않는다
    live: bool,
    lines: &'a mut Vec<String>,
    moving: &'a mut bool,
    items: &'a mut HashMap<String, f64>,
    nums: &'a mut HashMap<String, (f32, f32, f64)>,
    /// 웹 카드가 차지한 자리 (브라우저 화면을 그 위에 올린다)
    webs: &'a mut Vec<WebSlot>,
    /// 카드를 끌거나 크기를 바꾸는 중인가
    lifted: bool,
}

impl Ctx<'_> {
    /// 항목 하나를 그릴 붓. 새로 생긴 항목은 서서히 나타난다. 두 번째 값은 아래에서 올라오는 거리.
    fn row(&mut self, p: &Painter, item: &str) -> (Painter, f32) {
        let now = self.now;
        let t0 = *self.items.entry(item.to_string()).or_insert(now);
        let t = unit(now, t0, T_ITEM);
        if t < 1.0 {
            *self.moving = true;
        }
        let e = ease_out_cubic(t);
        let mut q = p.clone();
        q.set_opacity(self.alpha * e);
        (q, (1.0 - e) * 8.0)
    }
    /// 숫자 값이 바뀌면 이전 값에서 새 값으로 부드럽게 간다.
    fn num(&mut self, key: &str, target: f32) -> f32 {
        let now = self.now;
        let e = self.nums.entry(key.to_string()).or_insert((0.0, target, now));
        if (e.1 - target).abs() > 1e-4 {
            let cur = e.0 + (e.1 - e.0) * ease_out_cubic(unit(now, e.2, T_NUM));
            *e = (cur, target, now);
        }
        let t = unit(now, e.2, T_NUM);
        if t < 1.0 {
            *self.moving = true;
        }
        e.0 + (e.1 - e.0) * ease_out_cubic(t)
    }
    fn click(&self, rect: Rect, salt: (&str, &str)) -> Option<egui::Response> {
        self.live.then(|| self.ui.interact(rect, self.id.with(salt), Sense::click()).on_hover_cursor(CursorIcon::PointingHand))
    }
}

/// 보드 전체를 그린다. `lines` 에 사용자의 조작이 수정 언어로 쌓이고,
/// `clicked` 에 사용자가 짚은 카드가 들어간다. 반환값은 "아직 움직이는 중인가".
pub fn board(
    ui: &mut egui::Ui,
    board: &Board,
    view: &mut View,
    selected: &Option<String>,
    pending: &HashSet<String>,
    lines: &mut Vec<String>,
    clicked: &mut Option<String>,
    hot_zone: Option<&str>,
    webs: &mut Vec<WebSlot>,
) -> bool {
    let now = ui.input(|i| i.time);
    let mut moving = false;

    // 보드에서 사라진 카드는 바로 지우지 않고 서서히 사라지게 한다.
    for (id, v) in view.cards.iter_mut() {
        if board.card(id).is_none() && v.dying.is_none() {
            v.dying = Some(now);
        }
    }
    view.cards.retain(|_, v| v.dying.map(|t| now - t < T_OUT).unwrap_or(true));

    {
        // 하나의 넓은 판. 스크롤로 확대·축소하고 빈 곳을 끌어 옮긴다 (그래프 보기처럼).
        // 멀리서 보면 분류 덩어리와 이름만, 가까이 가면 주제, 더 가까이 가면 카드 내용이 보인다.
        let full_area = ui.available_rect_before_wrap();

        // "내 차례": 판 위가 아니라 화면 오른쪽에 고정된다 (확대·축소와 상관없이 항상 같은 자리, 같은 크기).
        let (inbox_slots, inbox_zones) = layout(board, INBOX_COLS as f32 * (CW + GAP) - GAP + 2.0 * ZONE_PAD, view, Some(NO_GROUP), true);
        let inbox = inbox_zones.into_iter().find(|z| z.inbox);
        // "내 차례" 구역이 오른쪽을 가리므로, 내용은 그 왼쪽 자리에 맞춘다.
        let covered = inbox.as_ref().map(|z| z.size.x + ZONE_GAP).unwrap_or(0.0);
        // 판은 "내 차례" 구역의 왼쪽만 쓴다. 판 위의 내용이 그 구역 밑으로 들어가 가려지는 일이 없다.
        let area = Rect::from_min_max(full_area.min, pos2((full_area.right() - covered).max(full_area.left() + 200.0), full_area.bottom()));
        let open = area.size();
        let (mut slots, zones, regions, bounds) = world(board, view, open.x - 8.0);
        slots.extend(inbox_slots);
        // 지금 화면에 없는 카드의 전환 상태는 버린다.
        view.cards.retain(|id, v| v.dying.is_some() || slots.contains_key(id));

        // 왼쪽의 열린 자리에 맞춘 범위를, 가려진 오른쪽만큼 넓혀서 실제로 볼 범위로 만든다.
        let widen = |r: Rect| Rect::from_min_size(r.min, vec2(r.width() * area.width() / open.x, r.height()));
        let everything = if bounds.width() <= open.x - 20.0 && bounds.height() <= open.y - 20.0 {
            Rect::from_min_size(bounds.min - vec2(4.0, 4.0), area.size()) // 다 들어오면 제 크기로, 왼쪽 위부터
        } else {
            // 세로로 남으면 가운데가 아니라 위쪽에 붙인다.
            let f = frame(bounds.expand(30.0), open, 1.0);
            let lift = ((f.height() - bounds.height()) * 0.5 - 30.0).max(0.0);
            widen(f.translate(vec2(0.0, lift)))
        };
        let mut scene = view.scene.unwrap_or(Rect::NOTHING);
        if !(scene.is_finite() && scene.width() > 1.0) {
            scene = everything;
            // 확인용: LYBOARD_ZOOM=0.3 처럼 주면 그 비율로 왼쪽 위부터 보여 준다.
            if let Some(z) = std::env::var("LYBOARD_ZOOM").ok().and_then(|v| v.parse::<f32>().ok()) {
                scene = Rect::from_min_size(bounds.min - vec2(20.0, 20.0), area.size() / z.clamp(0.05, 1.0));
            }
        }
        // AI 가 방금 고친 주제가 화면 밖에 있거나 너무 작게 보이면 그쪽으로 날아간다.
        if let Some(topic) = view.goto.take() {
            if let Some(z) = zones.iter().find(|z| z.id == topic) {
                let zr = Rect::from_min_size(z.min.to_pos2(), z.size);
                let zoom_now = area.width() / scene.width();
                if !scene.contains_rect(zr) || zoom_now < 0.55 {
                    // 구역이 커서 한 화면에 안 들어오면, 읽을 수 있는 크기를 지키고 구역의 왼쪽 위부터 보여 준다.
                    let fitted = frame(zr.expand(40.0), open, 1.0);
                    let target = if open.x / fitted.width() < READABLE { Rect::from_min_size(zr.min - vec2(30.0, 30.0), open / READABLE) } else { fitted };
                    view.fly = Some((scene, widen(target), now));
                }
            }
        }
        if let Some((from, to, t0)) = view.fly {
            // 튕겨 돌아올 때는 목표를 살짝 지나쳤다가 제자리로 온다.
            let t = unit(now, t0, if view.bounce { T_BOUNCE } else { T_FLY });
            let k = if view.bounce { ease_out_back(t) } else { ease_in_out_cubic(t) };
            scene = Rect::from_min_max(from.min.lerp(to.min, k), from.max.lerp(to.max, k));
            moving = true;
            if t >= 1.0 {
                view.fly = None;
                view.bounce = false;
            }
        }
        let zoom = (area.width() / scene.width()).clamp(0.01, 4.0);
        // 이보다 더 축소하면 새로 보이는 것이 없다 (전체가 이미 다 보인다).
        let fit_zoom = (area.width() / everything.width()).min(1.0);
        // 확대 정도에 따라 무엇까지 보일지


        // 카드는 이만큼 가까워져야 보인다. 그 전에는 주제 이름만 보인다.
        let cards_seen = smooth(zoom, 0.40, 0.48);

        let mut busy = false;
        let webs_start = webs.len();
        let scene_now = scene;
        let mut draw_cards = |ui: &mut egui::Ui, sticky_pass: bool, origin: Pos2, zoom: f32| {
            // 멀어질수록 내용 → 제목 → 상자 순으로 사라진다.
            let detail = smooth(zoom, 0.62, 0.78);
            let title_a = smooth(zoom, 0.50, 0.60);
            let box_a = smooth(zoom, 0.40, 0.48);
            let handy = detail > 0.6; // 이만큼 가까워야 카드를 누르고 끌 수 있다
        // 그리는 순서: 사라지는 카드 → 보통 카드 → 끌고 있는 카드
        let mut order: Vec<(u8, String)> = view
            .cards
            .iter()
            .filter(|(_, v)| v.dying.is_some() && v.sticky == sticky_pass)
            .map(|(id, _)| (0, id.clone()))
            .chain(board.cards.iter().filter(|c| slots.get(&c.id).map(|s| s.sticky == sticky_pass).unwrap_or(false)).map(|c| {
                let lifted = view.cards.get(&c.id).map(|v| v.dragging || v.resizing).unwrap_or(false);
                (if lifted { 2 } else { 1 }, c.id.clone())
            }))
            .collect();
        order.sort_by_key(|o| o.0);

        for (_, cid) in order {
            let live = board.card(&cid);
            if let Some(c) = live {
                let s = &slots[&c.id];
                let (w, h) = (s.w as f32, s.h as f32);
                let target_pos = s.pos;
                let target_size = vec2(w * CW + (w - 1.0) * GAP, h * CH + (h - 1.0) * GAP);
                let v = view.cards.entry(c.id.clone()).or_insert_with(|| CardView {
                    pos: Tween::fixed(target_pos),
                    size: Tween::fixed(target_size),
                    born: now,
                    rev: c.rev,
                    flash: now,
                    dragging: false,
                    resizing: false,
                    snap: c.clone(),
                    dying: None,
                    sticky: false,
                    content_h: 0.0,
                    items: HashMap::new(),
                    nums: HashMap::new(),
                });
                v.dying = None;
                if c.rev != v.rev {
                    v.rev = c.rev;
                    v.flash = now;
                }
                if !v.dragging {
                    v.pos.retarget(target_pos, now);
                }
                if !v.resizing {
                    v.size.retarget(target_size, now);
                }
                v.snap = c.clone();
                v.sticky = s.sticky;
            }
            let Some(v) = view.cards.get_mut(&cid) else { continue };
            let c = v.snap.clone();

            // 등장: 아래에서 살짝 올라오며 나타남. 퇴장: 살짝 줄어들며 사라짐.
            let t_in = ease_out_cubic(unit(now, v.born, T_IN));
            let t_out = v.dying.map(|t| ease_in_cubic(unit(now, t, T_OUT))).unwrap_or(0.0);
            let alpha = t_in * (1.0 - t_out);
            let flash = 1.0 - ease_out_cubic(unit(now, v.flash, T_FLASH));
            if v.pos.busy(now) || v.size.busy(now) || t_in < 1.0 || v.dying.is_some() || flash > 0.0 {
                busy = true;
            }

            let full = Rect::from_min_size(origin + v.pos.at(now), v.size.at(now));
            let scale = 0.97 + 0.03 * t_in - 0.04 * t_out;
            let rect = Rect::from_center_size(full.center(), full.size() * scale).translate(vec2(0.0, (1.0 - t_in) * 12.0));
            let id = ui.scope_id().with(("card", &c.id));
            let alive = live.is_some();

            // ---- 직접 조작 ----
            let mut hovered = false;
            if alive && handy {
                let s = &slots[&c.id];
                let (col, row, w) = (s.col, s.row, s.w);
                let body_resp = ui.interact(rect, id, Sense::click());
                hovered = body_resp.hovered();
                let head_rect = Rect::from_min_size(rect.min, vec2(rect.width() - 30.0, HEAD));
                let head = ui.interact(head_rect, id.with("head"), Sense::click_and_drag()).on_hover_cursor(CursorIcon::Grab);
                if head.drag_started() {
                    v.dragging = true;
                }
                if head.dragged() {
                    let p = v.pos.at(now) + head.drag_delta();
                    v.pos = Tween::fixed(p);
                    busy = true;
                }
                if head.drag_stopped() {
                    v.dragging = false;
                    // 놓은 자리를 그 주제 구역 안의 가장 가까운 칸으로 맞춘다.
                    let p = v.pos.at(now) - s.origin;
                    let ncol = ((p.x / (CW + GAP)).round() as i32).clamp(0, (s.limit - w).max(0));
                    let nrow = ((p.y / (CH + GAP)).round() as i32).max(0);
                    lines.push(format!("move {} {} {}", c.id, ncol, nrow));
                }
                if body_resp.clicked() || head.clicked() {
                    *clicked = Some(c.id.clone());
                }
                let grip = Rect::from_min_max(rect.max - vec2(22.0, 22.0), rect.max);
                let rs = ui.interact(grip, id.with("grip"), Sense::drag()).on_hover_cursor(CursorIcon::ResizeNwSe);
                if rs.dragged() {
                    v.resizing = true;
                    let s = (v.size.at(now) + rs.drag_delta()).max(vec2(CW, CH));
                    v.size = Tween::fixed(s);
                    busy = true;
                }
                if rs.drag_stopped() {
                    v.resizing = false;
                    let s = v.size.at(now);
                    let nw = (((s.x + GAP) / (CW + GAP)).round() as i32).clamp(1, 8);
                    let nh = (((s.y + GAP) / (CH + GAP)).round() as i32).clamp(1, 8);
                    if c.grid.is_none() {
                        lines.push(format!("move {} {} {}", c.id, col, row)); // 지금 자리에 고정한 뒤 크기를 바꾼다
                    }
                    lines.push(format!("size {} {}x{}", c.id, nw, nh));
                }
                if c.pinned {
                    let pin = Rect::from_center_size(pos2(rect.right() - 20.0, rect.top() + 25.0), vec2(20.0, 20.0));
                    let pr = ui.interact(pin, id.with("pin"), Sense::click()).on_hover_text("고정 해제 (자동 배치로 되돌림)");
                    if pr.clicked() {
                        lines.push(format!("unpin {}", c.id));
                    }
                }
            }

            // ---- 그리기 ----
            let mut p = ui.painter().with_clip_rect(rect.expand(40.0).intersect(ui.clip_rect()));
            p.set_opacity(alpha * box_a);
            let is_sel = selected.as_deref() == Some(c.id.as_str());
            let is_focus = board.focus.as_deref() == Some(c.id.as_str());
            let lifted = v.dragging || v.resizing;
            let radius = CornerRadius::same(RADIUS);
            let shadow = egui::epaint::Shadow {
                offset: [0, if lifted { 14 } else { 6 }],
                blur: if lifted { 36 } else { 22 },
                spread: 0,
                color: Color32::from_black_alpha(if lifted { 130 } else { 70 }),
            };
            p.add(shadow.as_shape(rect, radius));
            p.rect_filled(rect, radius, if lifted { mix(CARD, RAISED, 0.5) } else { CARD });
            let rest = if is_sel { TEXT } else if is_focus { EDGE } else { BORDER };
            p.rect_stroke(rect, radius, Stroke::new(if is_sel { 1.5 } else { 1.0 }, mix(rest, TEXT, flash * 0.55)), StrokeKind::Inside);

            // 사용자가 이 카드를 조작했고 AI 가 아직 반응하는 중이면 테두리에 주황색 선이 돈다.
            let (pk, pbusy) = tween_num(&mut v.nums, "pend", if pending.contains(&c.id) { 1.0 } else { 0.0 }, now, 0.3);
            if pk > 0.01 || pbusy {
                busy = true;
                draw_runner(&p, rect, RADIUS as f32, now, pk);
            }

            // 제목 오른쪽에 붙는 것(주제 이름, 완료 개수, 고정 표시)의 폭만큼 제목 자리를 줄여서 글자가 겹치지 않게 한다.
            let topic_label = if c.kind == Kind::Decision { board.topics.iter().find(|t| t.id == c.topic).map(|t| t.name.clone()).unwrap_or_default() } else { String::new() };
            let reserved = if !topic_label.is_empty() {
                p.layout_no_wrap(topic_label.clone(), font(11.0), DIM).size().x + 14.0
            } else if c.kind == Kind::Checklist && !c.items.is_empty() {
                64.0
            } else {
                0.0
            } + if c.pinned { 16.0 } else { 0.0 };
            let title_clip = Rect::from_min_max(rect.min, pos2(rect.right() - PAD - reserved, rect.top() + HEAD));
            let side_clip = Rect::from_min_max(rect.min, pos2(rect.right() - 6.0, rect.top() + HEAD));
            let mut tp = p.with_clip_rect(title_clip.intersect(p.clip_rect()));
            tp.set_opacity(alpha * title_a);
            tp.text(rect.min + vec2(PAD, 15.0), Align2::LEFT_TOP, &c.title, bold(16.0), TEXT);
            // 제목 옆에 붙는 것들은 제목과 다른 자리에 그린다.
            let mut sp = p.with_clip_rect(side_clip.intersect(p.clip_rect()));
            sp.set_opacity(alpha * title_a);
            // "내 차례" 구역의 카드는 주제 구역 밖에 있으므로, 어느 주제의 질문인지 제목 옆에 적는다.
            if c.kind == Kind::Decision {
                let topic = board.topics.iter().find(|t| t.id == c.topic).map(|t| t.name.as_str()).unwrap_or("");
                sp.text(pos2(rect.right() - if c.pinned { 34.0 } else { PAD }, rect.top() + 19.0), Align2::RIGHT_TOP, topic, font(11.0), ORANGE.gamma_multiply(0.85));
            }
            let right_x = rect.right() - if c.pinned { 34.0 } else { PAD };
            if c.kind == Kind::Checklist && !c.items.is_empty() {
                // 체크리스트: 완료 개수와 채워지는 고리. 체크하는 즉시 바뀐다.
                let total = c.items.len();
                let done = c.items.iter().filter(|(_, cells)| cells[0] == "done").count();
                let e = v.nums.entry("ring".into()).or_insert((0.0, 0.0, now));
                let target = done as f32 / total as f32;
                if (e.1 - target).abs() > 1e-4 {
                    *e = (e.0 + (e.1 - e.0) * ease_out_cubic(unit(now, e.2, T_NUM)), target, now);
                }
                let k = unit(now, e.2, T_NUM);
                if k < 1.0 {
                    busy = true;
                }
                let frac = e.0 + (e.1 - e.0) * ease_out_cubic(k);
                let all = done == total;
                let col = if all { GREEN } else { SOFT };
                let center = pos2(right_x - 8.0, rect.top() + 24.0);
                sp.circle_stroke(center, 7.0, Stroke::new(2.0, RAISED));
                let n = 40;
                let pts: Vec<Pos2> = (0..=((n as f32 * frac) as usize))
                    .map(|i| {
                        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * i as f32 / n as f32;
                        center + vec2(a.cos(), a.sin()) * 7.0
                    })
                    .collect();
                if pts.len() > 1 {
                    sp.line(pts, Stroke::new(2.0, col));
                }
                sp.text(pos2(right_x - 22.0, rect.top() + 24.0), Align2::RIGHT_CENTER, format!("{done}/{total}"), bold(12.5), if all { GREEN } else { DIM });
            }
            if c.pinned {
                p.circle_filled(pos2(rect.right() - 20.0, rect.top() + 25.0), 3.0, DIM);
            }
            if hovered || lifted {
                let m = rect.max - vec2(9.0, 9.0);
                for d in [4.0, 8.0] {
                    p.line_segment([m - vec2(d, 0.0), m - vec2(0.0, d)], Stroke::new(1.0, FAINT));
                }
            }

            let body = Rect::from_min_max(rect.min + vec2(PAD, HEAD), rect.max - vec2(PAD, 12.0));
            if body.height() > 4.0 && detail > 0.02 {
                let mut bp = p.with_clip_rect(body.expand2(vec2(2.0, 0.0)).intersect(p.clip_rect()));
                bp.set_opacity(alpha * detail);
                let mut cx = Ctx {
                    ui,
                    now,
                    id,
                    alpha: alpha * detail,
                    live: alive && handy,
                    lines,
                    moving: &mut busy,
                    items: &mut v.items,
                    nums: &mut v.nums,
                    webs: &mut *webs,
                    lifted,
                };
                let used = draw_body(&mut cx, &bp, body, &c);
                if alive {
                    v.content_h = used;
                }
                if used > body.height() + 3.0 {
                    // 내용이 카드보다 길다는 표시
                    for i in 0..3 {
                        p.circle_filled(pos2(rect.center().x - 6.0 + i as f32 * 6.0, rect.bottom() - 8.0), 1.3, DIM);
                    }
                }
            }
        }
        };

        // ---- 판: 분류 영역, 주제 구역, 카드 ----
        let parent_layer = ui.layer_id();
        let shown = ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| egui::Scene::new().zoom_range((fit_zoom * 0.86).max(MIN_ZOOM)..=1.0).scroll_zooms(true).show(ui, &mut scene, |ui| {
            let base = ui.painter().clone();
            // 확대 정도에 따라 세 단계로 보인다. 단계끼리 글자가 겹치지 않게, 한 단계의 이름이 사라진 뒤에 다음 단계가 나타난다.
            //   멀리 (far)   분류 이름만 크게
            //   중간 (mid)   주제 이름만 크게
            //   가까이       카드
            let far = 1.0 - smooth(zoom, 0.14, 0.18);
            let mid = smooth(zoom, 0.16, 0.20) * (1.0 - cards_seen);
            // 한 줄 글자가 주어진 폭에 들어가는 가장 큰 크기 (한글 한 글자의 폭을 글자 크기와 같다고 본다).
            let fit = |name: &str, width: f32, want: f32| want.min(width / (name.chars().count().max(1) as f32 * 1.05)).max(8.0);
            // 분류 영역: 넓은 바탕. 이름은 영역 바로 위에 적어서 안쪽 내용과 겹치지 않는다.
            for r in &regions {
                if r.name.is_empty() {
                    continue;
                }
                base.rect_filled(r.rect, 44.0, mix(BG, ZONE, 0.35 + 0.65 * far));
                base.rect_stroke(r.rect, 44.0, Stroke::new((1.0 / zoom).min(8.0), mix(ZONE_EDGE, EDGE, far * 0.6)), StrokeKind::Inside);
                // 이 분류 안에서 내 답을 기다리는 수: 영역 위 오른쪽 끝
                let rad = (13.0 / zoom).clamp(13.0, REGION_GAP * 0.3);
                let badge = if r.asks > 0 { rad * 2.0 + 12.0 / zoom } else { 0.0 };
                if r.asks > 0 {
                    let c = pos2(r.rect.right() - rad - 8.0, r.rect.top() - rad - 6.0 / zoom);
                    base.circle_filled(c, rad, ORANGE);
                    base.text(c, Align2::CENTER_CENTER, format!("{}", r.asks), bold(rad * 1.1), CARD);
                }
                if far < 0.55 {
                    // 가까이·중간: 영역 위 왼쪽에 이름 (화면에서 늘 같은 크기로 보이게 확대 비율만큼 키운다)
                    let size = fit(&r.name, r.rect.width() - badge - 16.0, (24.0 / zoom).min(REGION_GAP * 0.6));
                    let mut lp = base.clone();
                    lp.set_opacity((1.0 - far * 1.8).max(0.0)); // 가운데 이름이 나타나기 전에 먼저 사라진다
                    lp.text(pos2(r.rect.left() + 8.0, r.rect.top() - 6.0 / zoom), Align2::LEFT_BOTTOM, &r.name, bold(size), SOFT);
                }
                if far > 0.45 {
                    // 멀리: 영역 한가운데에 이름을 크게. 이때 주제 이름은 보이지 않는다.
                    let size = fit(&r.name, r.rect.width() * 0.8, (30.0 / zoom).min(r.rect.height() * 0.35));
                    let mut lp = base.clone();
                    lp.set_opacity((far * 1.8 - 0.8).max(0.0));
                    lp.text(r.rect.center(), Align2::CENTER_CENTER, &r.name, bold(size), TEXT);
                }
            }
            // 주제 구역: 이름이 붙은 바탕 판. 카드는 자기 주제의 판 위에만 놓인다.
            view.zones.retain(|id, _| zones.iter().any(|z| &z.id == id));
            for z in &zones {
                let zv = view.zones.entry(z.id.clone()).or_insert_with(|| (Tween::fixed(z.min), Tween::fixed(z.size), now));
                zv.0.retarget(z.min, now);
                zv.1.retarget(z.size, now);
                let appear = ease_out_cubic(unit(now, zv.2, T_IN));
                if zv.0.busy(now) || zv.1.busy(now) || appear < 1.0 {
                    moving = true;
                }
                let r = Rect::from_min_size(zv.0.at(now).to_pos2(), zv.1.at(now));
                let mut zp = base.clone();
                // 분류 이름만 보이는 단계에서는 주제 판이 옅은 덩어리로만 남는다.
                let grouped = !regions.is_empty();
                zp.set_opacity(appear * if grouped { 1.0 - far * 0.75 } else { 1.0 });
                zp.rect_filled(r, 26.0, mix(ZONE, RAISED, (1.0 - cards_seen) * 0.6));
                zp.rect_stroke(r, 26.0, Stroke::new((1.0 / zoom).min(5.0), ZONE_EDGE), StrokeKind::Inside);
                let count = board.cards.iter().filter(|c| c.topic == z.id && c.kind != Kind::Decision).count();
                if cards_seen > 0.02 {
                    // 가까이: 구역 위쪽에 이름과 카드 수 (카드가 보이는 단계)
                    let mut np = zp.with_clip_rect(Rect::from_min_size(r.min, vec2(r.width() - 8.0, ZONE_HEAD)).intersect(zp.clip_rect()));
                    np.set_opacity(appear * cards_seen);
                    // 조금 축소된 상태에서도 읽히게, 구역 머리 높이 안에서 글자를 키운다.
                    let name_size = (15.0 / zoom).clamp(15.0, 24.0);
                    let tr = np.text(r.min + vec2(ZONE_PAD + 8.0, (ZONE_HEAD - name_size) * 0.5 - 2.0), Align2::LEFT_TOP, &z.name, bold(name_size), SOFT);
                    np.text(pos2(tr.right() + 10.0, tr.center().y), Align2::LEFT_CENTER, format!("{count}"), font(12.0), FAINT);
                }
                let mid_here = if grouped { mid } else { 1.0 - cards_seen };
                if mid_here > 0.02 {
                    // 중간: 카드가 안 보이므로 구역 한가운데에 이름을 크게
                    let size = fit(&z.name, r.width() - 24.0, (16.0 / zoom).min(r.height() * 0.4));
                    let mut np = zp.with_clip_rect(r.shrink(6.0).intersect(zp.clip_rect()));
                    np.set_opacity(appear * mid_here);
                    np.text(r.center(), Align2::CENTER_CENTER, &z.name, bold(size), TEXT);
                }
                // 방금 한 말이 이 주제의 이야기라고 판단되면, 답이 오기 전부터 이 구역 테두리에 선이 돈다.
                if hot_zone == Some(z.id.as_str()) {
                    moving = true;
                    draw_runner(&zp, r, 26.0, now, 1.0);
                }
            }
            draw_cards(ui, false, Pos2::ZERO, zoom);
        })).inner;
        // 손으로 옮기거나 확대하면 자동 이동은 그만둔다. 빈 곳을 두 번 누르면 전체가 보이게 돌아간다.
        // 전체가 다 보이는 것보다 더 축소해 봐야 작아지기만 한다. 거기까지는 살짝 당겨지지만, 손을 떼면 튕겨 돌아온다.
        let pulling = ui.input(|i| i.smooth_scroll_delta.y.abs() > 0.1 || i.zoom_delta() != 1.0);
        if area.width() / scene.width() < fit_zoom * 0.995 && !pulling && view.fly.is_none() {
            view.fly = Some((scene, everything, now));
            view.bounce = true;
        } else if shown.response.changed() && !view.bounce {
            view.fly = None;
        }
        if shown.response.double_clicked() {
            view.fly = Some((scene, everything, now));
        }
        view.scene = Some(scene);
        view.zoom = zoom;

        // ---- "내 차례": 판 위에 겹쳐서 화면 오른쪽에 고정 ----
        if let Some(z) = &inbox {
            let over_layer = egui::LayerId::new(parent_layer.order, ui.scope_id().with("inbox_layer"));
            ui.ctx().set_sublayer(parent_layer, over_layer);
            let mut over = ui.new_child(egui::UiBuilder::new().layer_id(over_layer).max_rect(full_area).sense(Sense::hover()));
            over.set_clip_rect(full_area);
            // 답할 것이 많아 화면보다 길어지면, 이 구역 위에서 스크롤해 넘겨 본다.
            let reach = (z.size.y - full_area.height()).max(0.0);
            let home = Rect::from_min_size(pos2(full_area.right() - z.size.x, full_area.top()), vec2(z.size.x, full_area.height()));
            if ui.rect_contains_pointer(home) {
                view.inbox_scroll -= ui.input(|i| i.smooth_scroll_delta.y);
            }
            view.inbox_scroll = view.inbox_scroll.clamp(0.0, reach);
            let r = Rect::from_min_size(pos2(full_area.right() - z.size.x, full_area.top() - view.inbox_scroll), z.size);
            // 뒤의 판이 눌리거나 끌리지 않게 이 자리를 차지한다.
            over.interact(home, over.scope_id().with("inbox-bg"), Sense::click_and_drag());
            let zp = over.painter().clone();
            zp.rect_filled(r, 26.0, ZONE);
            // AI 가 답을 받으러 온 것들. 주황색 테두리와 점으로 눈에 띄게 한다.
            zp.rect_stroke(r, 26.0, Stroke::new(1.0, mix(ZONE_EDGE, ORANGE, 0.55)), StrokeKind::Inside);
            let count = board.cards.iter().filter(|c| c.kind == Kind::Decision).count();
            let pulse = ((now * 2.4).sin() * 0.5 + 0.5) as f32;
            let dot = r.min + vec2(ZONE_PAD + 13.0, 22.0);
            zp.circle_filled(dot, 4.0 + 3.0 * pulse, ORANGE.gamma_multiply(0.25 * (1.0 - pulse)));
            zp.circle_filled(dot, 4.0, ORANGE);
            let tr = zp.text(r.min + vec2(ZONE_PAD + 26.0, 13.0), Align2::LEFT_TOP, &z.name, bold(15.0), TEXT);
            zp.text(pos2(tr.right() + 10.0, tr.center().y), Align2::LEFT_CENTER, format!("{count}"), bold(12.5), ORANGE);
            moving = true;
            draw_cards(&mut over, true, r.min, 1.0);
        }
        drop(draw_cards);
        moving |= busy;
        // 판 위의 웹 카드 자리를 화면 좌표로 바꾼다. 축소된 상태에서는 브라우저 화면을 띄우지 않는다.
        for w in webs.iter_mut().skip(webs_start) {
            let to_screen = |p: Pos2| area.min + (p - scene_now.min) * zoom;
            w.rect = Rect::from_min_max(to_screen(w.rect.min), to_screen(w.rect.max));
            w.visible = w.visible && zoom > 0.97 && area.contains_rect(w.rect);
        }

        if board.cards.is_empty() {
            ui.painter().text(area.center(), Align2::CENTER_CENTER, "왼쪽에 말을 걸어 보세요", bold(20.0), FAINT);
        }
    }
    moving
}

/// 0~1 사이로 부드럽게 이어지는 계단: x 가 a 이하면 0, b 이상이면 1.
/// 목표를 조금 지나쳤다가 돌아오는 곡선 (easings.net 의 easeOutBack).
fn ease_out_back(t: f32) -> f32 {
    let (c1, c3) = (1.70158_f32, 2.70158_f32);
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

fn smooth(x: f32, a: f32, b: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `target` 이 다 들어오면서 화면과 가로세로 비율이 같은 범위. `max_zoom` 보다 더 확대하지는 않는다.
fn frame(target: Rect, view: Vec2, max_zoom: f32) -> Rect {
    let scale = (view.x / target.width().max(1.0)).min(view.y / target.height().max(1.0)).min(max_zoom);
    Rect::from_center_size(target.center(), view / scale)
}

/// 분류 하나가 판 위에서 차지하는 영역.
struct Region {
    name: String,
    rect: Rect,
    /// 이 분류 안에서 내 답을 기다리는 카드 수
    asks: usize,
}

const MIN_ZOOM: f32 = 0.06;
const T_FLY: f64 = 0.6;
const T_BOUNCE: f64 = 0.5;
const REGION_W: f32 = 1500.0;
const REGION_GAP: f32 = 200.0;
const REGION_PAD: f32 = 40.0;
/// 분류 이름은 영역 위에 적으므로 영역 안쪽 위 여백은 작다.
const REGION_HEAD: f32 = 40.0;
/// 주제로 날아갈 때 이보다 작게 축소하지 않는다 (카드 글자를 읽을 수 있는 크기).
const READABLE: f32 = 0.8;

/// 판 전체의 배치. 분류마다 영역을 하나씩 잡고, 그 안에 그 분류의 주제 구역들을 놓는다.
/// 분류가 하나도 없으면 영역 없이 주제 구역만 놓인다.
fn world(board: &Board, view: &View, plain_width: f32) -> (HashMap<String, Slot>, Vec<Zone>, Vec<Region>, Rect) {
    if board.groups.is_empty() {
        // 분류가 없을 때는 화면 폭에 맞춰 놓아서 제 크기 그대로 보이게 한다.
        let (slots, zones) = layout(board, plain_width, view, None, false);
        let max = zones.iter().fold(Vec2::ZERO, |m, z| m.max(z.min + z.size));
        return (slots, zones, Vec::new(), Rect::from_min_size(Pos2::ZERO, max));
    }
    let mut keys: Vec<(String, String)> = board.groups.iter().map(|g| (g.id.clone(), g.name.clone())).collect();
    keys.push((String::new(), "미분류".to_string()));
    let (mut slots, mut zones, mut regions) = (HashMap::new(), Vec::new(), Vec::new());
    // 맨 윗줄 분류의 이름이 놓일 자리를 위에 남긴다.
    let (mut x, mut y, mut row_h) = (0.0_f32, REGION_GAP * 0.7, 0.0_f32);
    // 화면이 가로로 넓으므로 분류를 한 줄에 셋까지 놓는다 (전체를 볼 때 덜 축소된다).
    let wrap_at = REGION_W * 3.0 + REGION_GAP * 2.0 + 6.0 * REGION_PAD;
    let mut max = Vec2::ZERO;
    for (gid, name) in keys {
        let (s, z) = layout(board, REGION_W, view, Some(gid.as_str()), false);
        if z.is_empty() {
            continue;
        }
        let content = z.iter().fold(Vec2::ZERO, |m, zz| m.max(zz.min + zz.size));
        let size = vec2(content.x.max(520.0) + 2.0 * REGION_PAD, content.y + REGION_HEAD + REGION_PAD);
        if x > 0.0 && x + size.x > wrap_at {
            x = 0.0;
            y += row_h + REGION_GAP;
            row_h = 0.0;
        }
        let rect = Rect::from_min_size(pos2(x, y), size);
        let shift = vec2(x + REGION_PAD, y + REGION_HEAD);
        let topic_ids: Vec<&str> = z.iter().map(|zz| zz.id.as_str()).collect();
        let asks = board.cards.iter().filter(|c| c.kind == Kind::Decision && board.topics.iter().any(|t| t.id == c.topic && t.group.as_deref().unwrap_or("") == gid)).count();
        let _ = topic_ids;
        for (id, mut slot) in s {
            slot.pos += shift;
            slot.origin += shift;
            slots.insert(id, slot);
        }
        for mut zz in z {
            zz.min += shift;
            zones.push(zz);
        }
        regions.push(Region { name, rect, asks });
        max = max.max(rect.max.to_vec2());
        x += size.x + REGION_GAP;
        row_h = row_h.max(size.y);
    }
    (slots, zones, regions, Rect::from_min_size(Pos2::ZERO, max))
}

fn wrapped(p: &Painter, pos: Pos2, width: f32, text: &str, size: f32, color: Color32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let g = p.layout(text.to_owned(), font(size), color, width.max(20.0));
    let h = g.size().y;
    p.galley(pos, g, color);
    h
}

fn check_mark(p: &Painter, c: Pos2, color: Color32) {
    let s = Stroke::new(1.7, color);
    p.line_segment([c + vec2(-3.6, 0.0), c + vec2(-1.0, 2.8)], s);
    p.line_segment([c + vec2(-1.0, 2.8), c + vec2(3.8, -2.8)], s);
}

fn state_color(s: &str) -> Color32 {
    match s {
        "ok" | "done" => GREEN,
        "run" => TEXT,
        "wait" => AMBER,
        "fail" => RED,
        _ => FAINT,
    }
}

fn state_dot(p: &Painter, c: Pos2, state: &str, now: f64, moving: &mut bool) {
    let col = state_color(state);
    match state {
        "done" | "ok" => {
            p.circle_filled(c, 7.0, col);
            check_mark(p, c, CARD);
        }
        "run" => {
            // 숨 쉬듯 퍼졌다 사라지는 고리
            *moving = true;
            let t = ((now * 0.8).fract()) as f32;
            let e = ease_out_cubic(t);
            p.circle_stroke(c, 4.0 + 6.0 * e, Stroke::new(1.2, col.gamma_multiply(0.5 * (1.0 - e))));
            p.circle_filled(c, 4.0, col);
        }
        "fail" => {
            p.circle_filled(c, 7.0, col);
            let s = Stroke::new(1.7, CARD);
            p.line_segment([c + vec2(-2.8, -2.8), c + vec2(2.8, 2.8)], s);
            p.line_segment([c + vec2(-2.8, 2.8), c + vec2(2.8, -2.8)], s);
        }
        "wait" => {
            p.circle_stroke(c, 6.0, Stroke::new(1.5, col));
            p.circle_filled(c, 2.0, col);
        }
        _ => {
            p.circle_stroke(c, 6.0, Stroke::new(1.4, col));
        }
    }
}

/// 부품 종류별 본문. 반환값은 내용이 실제로 차지한 높이.
fn draw_body(cx: &mut Ctx, p: &Painter, body: Rect, c: &Card) -> f32 {
    let (x, top, w) = (body.left(), body.top(), body.width());
    let now = cx.now;
    match c.kind {
        Kind::Note => wrapped(p, body.min, w, c.field("text"), 15.0, SOFT),

        Kind::Web => {
            // 이 자리 위에 실제 브라우저 화면이 올라간다. 여기서는 자리와 주소만 알려 준다.
            let url = c.field("url").to_string();
            p.rect_filled(body, 10.0, RAISED);
            let label = if url.is_empty() { "주소가 아직 없습니다" } else { url.as_str() };
            p.text(body.center(), Align2::CENTER_CENTER, label, font(12.0), FAINT);
            let shown = body.intersect(cx.ui.clip_rect());
            // 화면 밖으로 반쯤 나갔거나, 끌고 있거나, 나타나고 사라지는 중이면 브라우저 화면은 잠시 숨긴다.
            let visible = cx.live && !cx.lifted && cx.alpha > 0.98 && shown.height() > body.height() - 2.0 && shown.width() > body.width() - 2.0;
            cx.webs.push(WebSlot { id: c.id.clone(), rect: body, url, visible });
            body.height()
        }

        Kind::Status => {
            let state = c.field("state");
            let label = match state {
                "ok" => "정상",
                "run" => "진행 중",
                "wait" => "대기",
                "fail" => "실패",
                _ => "멈춤",
            };
            state_dot(p, pos2(x + 8.0, top + 12.0), state, now, cx.moving);
            p.text(pos2(x + 26.0, top - 1.0), Align2::LEFT_TOP, label, bold(18.0), state_color(state));
            let h = wrapped(p, pos2(x, top + 26.0), w, c.field("text"), 12.5, DIM);
            26.0 + h
        }

        Kind::Metric => {
            let r = p.text(pos2(x, top - 8.0), Align2::LEFT_TOP, c.field("value"), bold(46.0), TEXT);
            p.text(pos2(r.right() + 8.0, r.bottom() - 13.0), Align2::LEFT_BOTTOM, c.field("unit"), bold(12.0), DIM);
            let mut y = r.bottom() + 2.0;
            let delta = c.field("delta");
            if !delta.is_empty() {
                let col = if delta.starts_with('-') {
                    RED
                } else if delta.starts_with('+') {
                    GREEN
                } else {
                    DIM
                };
                p.text(pos2(x, y), Align2::LEFT_TOP, delta, bold(13.0), col);
                y += 21.0;
            }
            y += wrapped(p, pos2(x, y), w, c.field("text"), 12.5, DIM);
            y - top
        }

        Kind::Chart => {
            let vals: Vec<f32> =
                c.items.iter().map(|(_, cells)| cells.get(1).and_then(|v| v.replace(',', "").parse().ok()).unwrap_or(0.0)).collect();
            let max = vals.iter().cloned().fold(0.0_f32, f32::max).max(1e-6);
            let rh = 30.0;
            let label_w = c
                .items
                .iter()
                .map(|(_, cells)| p.layout_no_wrap(cells[0].clone(), bold(13.0), TEXT).size().x)
                .fold(0.0_f32, f32::max)
                .min(w * 0.38)
                + 16.0;
            let value_w = 70.0;
            let unit_s = c.field("unit").to_string();
            for (i, (item, cells)) in c.items.iter().enumerate() {
                let (rp, dy) = cx.row(p, item);
                let y = top + i as f32 * rh + dy;
                let lr = Rect::from_min_size(pos2(x, y), vec2(label_w - 10.0, rh));
                rp.with_clip_rect(lr.intersect(rp.clip_rect())).text(pos2(x, y + rh / 2.0), Align2::LEFT_CENTER, &cells[0], bold(13.0), TEXT);
                let track = Rect::from_min_size(pos2(x + label_w, y + rh / 2.0 - 3.0), vec2((w - label_w - value_w).max(10.0), 6.0));
                rp.rect_filled(track, 3.0, RAISED);
                let frac = cx.num(item, vals[i] / max);
                rp.rect_filled(Rect::from_min_size(track.min, vec2((track.width() * frac).max(6.0), 6.0)), 3.0, SOFT);
                rp.text(
                    pos2(body.right(), y + rh / 2.0),
                    Align2::RIGHT_CENTER,
                    format!("{}{}", cells.get(1).map(String::as_str).unwrap_or(""), unit_s),
                    font(13.0),
                    SOFT,
                );
            }
            c.items.len() as f32 * rh
        }

        Kind::Table => {
            let cols: Vec<&str> = c.field("cols").split('|').map(str::trim).filter(|s| !s.is_empty()).collect();
            let n = cols.len().max(c.items.iter().map(|(_, cells)| cells.len()).max().unwrap_or(1)).max(1);
            // 열 너비는 내용 길이에 맞춘다. 다 들어가면 남는 폭을 고르게 나누고, 넘치면 긴 열부터 줄인다.
            let mut want = vec![40.0_f32; n];
            for (i, name) in cols.iter().enumerate() {
                want[i] = want[i].max(p.layout_no_wrap(name.to_string(), font(11.0), FAINT).size().x);
            }
            for (_, cells) in &c.items {
                for (i, text) in cells.iter().enumerate().take(n) {
                    let f = if i == 0 { bold(13.5) } else { font(13.5) };
                    want[i] = want[i].max(p.layout_no_wrap(text.clone(), f, TEXT).size().x);
                }
            }
            let pad = 16.0;
            let avail = (w - pad * (n as f32 - 1.0)).max(40.0);
            let total: f32 = want.iter().sum();
            let widths: Vec<f32> = if total <= avail {
                let extra = (avail - total) / n as f32;
                want.iter().map(|x| x + extra).collect()
            } else {
                // 평균보다 짧은 열은 그대로 두고, 긴 열들이 남은 폭을 비율대로 나눠 갖는다.
                let fair = avail / n as f32;
                let short: f32 = want.iter().filter(|x| **x <= fair).sum();
                let long: f32 = want.iter().filter(|x| **x > fair).sum();
                want.iter().map(|x| if *x <= fair { *x } else { x / long * (avail - short) }).collect()
            };
            let lefts: Vec<f32> = widths.iter().scan(x, |acc, wd| {
                let l = *acc;
                *acc += wd + pad;
                Some(l)
            }).collect();
            let mut y = top;
            let cell = |q: &Painter, col: usize, y: f32, text: &str, f: FontId, color: Color32| {
                if col >= n {
                    return;
                }
                let r = Rect::from_min_size(pos2(lefts[col], y), vec2(widths[col], 28.0));
                q.with_clip_rect(r.intersect(q.clip_rect())).text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, text, f, color);
            };
            if !cols.is_empty() {
                for (i, name) in cols.iter().enumerate() {
                    cell(p, i, y - 4.0, name, font(11.0), FAINT);
                }
                y += 24.0;
            }
            // 첫 칸은 굵은 항목명, 나머지는 옅은 값
            for (item, cells) in &c.items {
                let (rp, dy) = cx.row(p, item);
                for (i, text) in cells.iter().enumerate() {
                    if i == 0 {
                        cell(&rp, i, y + dy, text, bold(13.5), TEXT);
                    } else {
                        cell(&rp, i, y + dy, text, font(13.5), DIM);
                    }
                }
                y += 28.0;
            }
            y - top
        }

        Kind::Progress => {
            let pct = cx.num("pct", c.field("pct").parse().unwrap_or(0.0));
            let r = p.text(pos2(x, top - 6.0), Align2::LEFT_TOP, format!("{pct:.0}%"), bold(30.0), TEXT);
            let tx = r.right() + 12.0;
            let tp = p.with_clip_rect(Rect::from_min_max(pos2(tx, top), pos2(body.right(), top + 30.0)).intersect(p.clip_rect()));
            tp.text(pos2(tx, r.bottom() - 9.0), Align2::LEFT_BOTTOM, c.field("text"), font(12.5), DIM);
            let track = Rect::from_min_size(pos2(x, top + 38.0), vec2(w, 6.0));
            p.rect_filled(track, 3.0, RAISED);
            p.rect_filled(Rect::from_min_size(track.min, vec2((w * pct / 100.0).max(6.0), 6.0)), 3.0, SOFT);
            let mut y = top + 58.0;
            for (item, cells) in &c.items {
                let (rp, dy) = cx.row(p, item);
                let state = cells[0].as_str();
                state_dot(&rp, pos2(x + 8.0, y + 13.0 + dy), state, now, cx.moving);
                let label = cells.get(1).map(String::as_str).unwrap_or("");
                if state == "run" {
                    rp.text(pos2(x + 26.0, y + 13.0 + dy), Align2::LEFT_CENTER, label, bold(13.5), TEXT);
                } else {
                    rp.text(pos2(x + 26.0, y + 13.0 + dy), Align2::LEFT_CENTER, label, font(13.5), DIM);
                }
                y += 28.0;
            }
            y - top
        }

        Kind::Checklist => {
            let mut y = top;
            for (item, cells) in &c.items {
                let (rp, dy) = cx.row(p, item);
                let done = cells[0] == "done";
                let label = cells.get(1).map(String::as_str).unwrap_or("");
                let row = Rect::from_min_size(pos2(x, y), vec2(w, 28.0));
                let r = cx.click(row, ("chk", item));
                let hover = r.as_ref().map(|r| r.hovered()).unwrap_or(false);
                if r.map(|r| r.clicked()).unwrap_or(false) {
                    cx.lines.push(format!("upd {} {} {} | {}", c.id, item, if done { "todo" } else { "done" }, label));
                }
                // 체크 상태가 바뀌면 채움이 부드럽게 들어오고 나간다.
                let fill = cx.num(&format!("chk:{item}"), if done { 1.0 } else { 0.0 });
                let bx = Rect::from_center_size(pos2(x + 9.0, y + 14.0 + dy), vec2(17.0, 17.0));
                rp.rect_stroke(bx, 5.0, Stroke::new(1.3, if hover { TEXT } else { mix(EDGE, SOFT, fill) }), StrokeKind::Inside);
                if fill > 0.01 {
                    rp.rect_filled(bx.shrink(8.5 * (1.0 - fill)), 5.0, SOFT);
                    if fill > 0.6 {
                        check_mark(&rp, bx.center(), CARD);
                    }
                }
                rp.text(pos2(x + 28.0, y + 14.0 + dy), Align2::LEFT_CENTER, label, font(13.5), mix(TEXT, FAINT, fill));
                y += 28.0;
            }
            y - top
        }

        Kind::Decision => {
            let mut y = top + wrapped(p, body.min, w, c.field("text"), 14.5, SOFT) + 12.0;
            let chosen = c.field("chosen").to_string();
            for (item, cells) in &c.items {
                let (rp, dy) = cx.row(p, item);
                let has_desc = cells.len() > 1 && !cells[1].is_empty();
                let h = if has_desc { 52.0 } else { 36.0 };
                let r = Rect::from_min_size(pos2(x, y + dy), vec2(w, h));
                let resp = cx.click(r, ("opt", item));
                let hover = resp.as_ref().map(|r| r.hovered()).unwrap_or(false);
                if resp.map(|r| r.clicked()).unwrap_or(false) {
                    cx.lines.push(format!("set {} chosen {}", c.id, item));
                }
                // 테두리만 있는 알약. 고르면 밝게 채워진다.
                let on = cx.num(&format!("opt:{item}"), if chosen == *item { 1.0 } else { 0.0 });
                let glow = cx.num(&format!("hov:{item}"), if hover { 1.0 } else { 0.0 });
                let rad = if has_desc { 16.0 } else { 18.0 };
                rp.rect_filled(r, rad, mix(mix(CARD, RAISED, glow), SOFT, on));
                rp.rect_stroke(r, rad, Stroke::new(1.0, mix(mix(BORDER, EDGE, glow), SOFT, on)), StrokeKind::Inside);
                let inner = rp.with_clip_rect(r.shrink2(vec2(14.0, 0.0)).intersect(rp.clip_rect()));
                let (c1, c2) = (mix(TEXT, CARD, on), mix(DIM, RAISED, on));
                if has_desc {
                    inner.text(pos2(x + 16.0, y + dy + 9.0), Align2::LEFT_TOP, &cells[0], bold(14.0), c1);
                    inner.text(pos2(x + 16.0, y + dy + 29.0), Align2::LEFT_TOP, &cells[1], font(12.0), c2);
                } else {
                    inner.text(pos2(x + 16.0, y + dy + h / 2.0), Align2::LEFT_CENTER, &cells[0], bold(14.0), c1);
                }
                y += h + 8.0;
            }
            y - 8.0 - top
        }

        Kind::Timeline => {
            let n = c.items.len();
            let rh = 30.0;
            if n > 1 {
                p.line_segment([pos2(x + 6.0, top + rh / 2.0), pos2(x + 6.0, top + rh * (n as f32 - 0.5))], Stroke::new(1.2, BORDER));
            }
            let when_w = c
                .items
                .iter()
                .map(|(_, cells)| p.layout_no_wrap(cells[0].clone(), bold(12.5), TEXT).size().x)
                .fold(0.0_f32, f32::max)
                .min(w * 0.4);
            for (i, (item, cells)) in c.items.iter().enumerate() {
                let (rp, dy) = cx.row(p, item);
                let y = top + i as f32 * rh + rh / 2.0 + dy;
                let state = cells.get(2).map(String::as_str).unwrap_or("todo");
                rp.circle_filled(pos2(x + 6.0, y), 6.0, CARD);
                match state {
                    "done" => {
                        rp.circle_filled(pos2(x + 6.0, y), 4.5, SOFT);
                    }
                    "run" => state_dot(&rp, pos2(x + 6.0, y), "run", now, cx.moving),
                    _ => {
                        rp.circle_stroke(pos2(x + 6.0, y), 4.0, Stroke::new(1.4, EDGE));
                    }
                }
                // 굵은 시점 + 옅은 내용
                rp.text(pos2(x + 22.0, y), Align2::LEFT_CENTER, &cells[0], bold(12.5), if state == "done" { DIM } else { TEXT });
                rp.text(
                    pos2(x + 36.0 + when_w, y),
                    Align2::LEFT_CENTER,
                    cells.get(1).map(String::as_str).unwrap_or(""),
                    font(13.5),
                    if state == "done" { FAINT } else { SOFT },
                );
            }
            n as f32 * rh
        }
    }
}
