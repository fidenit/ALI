//! 화면 트리(보드)와 조작. 모델·사용자·규칙이 만든 모든 변화는 `Op` 하나로 표현되고
//! `Board::apply` 가 타입 검사를 한 뒤 반영한다.

use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Note,
    Status,
    Progress,
    Decision,
    /// 사용자에게서 값(날짜, 이름, 숫자)을 직접 받아야 할 때: 칸에 적고 보내는 카드
    Input,
    Checklist,
    Table,
    Metric,
    Chart,
    Timeline,
    /// 웹 페이지나 HTML 파일을 그대로 보여 주는 작은 브라우저
    Web,
}

pub const KINDS: [(Kind, &str, (u8, u8)); 11] = [
    (Kind::Web, "web", (6, 5)),
    (Kind::Note, "note", (3, 2)),
    (Kind::Status, "status", (2, 1)),
    (Kind::Progress, "progress", (3, 3)),
    (Kind::Decision, "decision", (3, 3)),
    (Kind::Input, "input", (3, 3)),
    (Kind::Checklist, "checklist", (3, 3)),
    (Kind::Table, "table", (4, 3)),
    (Kind::Metric, "metric", (2, 2)),
    (Kind::Chart, "chart", (4, 3)),
    (Kind::Timeline, "timeline", (3, 3)),
];

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        KINDS.iter().find(|k| k.1 == s).map(|k| k.0)
    }
    pub fn name(self) -> &'static str {
        KINDS.iter().find(|k| k.0 == self).map(|k| k.1).unwrap_or("?")
    }
    pub fn default_size(self) -> (u8, u8) {
        KINDS.iter().find(|k| k.0 == self).map(|k| k.2).unwrap_or((3, 2))
    }
    /// 사용자의 답을 기다리는 카드인가 ("내 차례"로 가져갈 것)
    pub fn asks(self) -> bool {
        matches!(self, Kind::Decision | Kind::Input)
    }
    pub fn has_items(self) -> bool {
        !matches!(self, Kind::Note | Kind::Status | Kind::Metric | Kind::Web)
    }
    /// 필드별 글자 수 한도. 이 종류에 없는 필드면 None.
    /// 글이 들어가는 자리를 좁게 잡는 것이 이 프로그램의 핵심 제약이다.
    pub fn field_limit(self, field: &str) -> Option<usize> {
        use Kind::*;
        Some(match (self, field) {
            (_, "title") => 40,
            (Note, "text") => 240,
            (Status, "state") => 8,
            (Status, "text") => 80,
            (Progress, "pct") => 3,
            (Progress, "text") => 60,
            (Decision, "text") => 120,
            (Decision, "chosen") => 24,
            (Input, "text") => 120,
            (Input, "sent") => 8,
            (Table, "cols") => 200,
            (Metric, "value") => 14,
            (Metric, "unit") => 8,
            (Metric, "delta") => 14,
            (Metric, "text") => 60,
            (Chart, "unit") => 8,
            (Web, "url") => 600,
            _ => return None,
        })
    }
}

pub const MAX_ITEMS: usize = 24;
pub const MAX_CELLS: usize = 6;
pub const CELL_LIMIT: usize = 60;
pub const MAX_W: u8 = 8;
pub const MAX_H: u8 = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Author {
    Ai,
    User,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Topic { id: String, name: String },
    /// 분류를 만들거나 이름을 바꾼다
    Group { id: String, name: String },
    /// 카드를 다른 주제로 옮긴다 (주제를 합칠 때 쓴다)
    Mv { card: String, topic: String },
    /// 주제를 분류에 넣는다
    Put { topic: String, group: String },
    Card { id: String, kind: String, topic: String, size: Option<(u8, u8)>, title: String },
    Set { id: String, field: String, value: String },
    Add { id: String, item: String, cells: Vec<String> },
    Upd { id: String, item: String, cells: Vec<String> },
    Del { id: String, item: Option<String> },
    Size { id: String, w: u8, h: u8 },
    Move { id: String, col: i32, row: i32 },
    Pin { id: String, on: bool },
    Focus { id: String },
    Say { text: String },
    /// 주제를 카드째로 보관함에 내린다 / 다시 꺼낸다
    Archive { topic: String },
    Restore { topic: String },
}

/// 분류: 주제들을 묶는 한 단계 위의 묶음 ("사적", "회사" 같은 넓은 영역).
#[derive(Clone, Debug)]
pub struct Group {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Topic {
    pub id: String,
    pub name: String,
    /// 이 주제가 속한 분류. 없으면 아직 분류되지 않은 주제.
    pub group: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub id: String,
    pub kind: Kind,
    pub topic: String,
    pub title: String,
    pub fields: BTreeMap<String, String>,
    pub items: Vec<(String, Vec<String>)>,
    pub w: u8,
    pub h: u8,
    /// 명시적 위치(격자 칸). None 이면 배치 엔진이 정한다.
    pub grid: Option<(i32, i32)>,
    /// 사용자가 직접 옮기거나 크기를 바꾼 카드. AI 는 위치·크기를 덮어쓸 수 없다.
    pub pinned: bool,
    /// 바뀔 때마다 1씩 증가. 화면이 변화 강조에 쓴다.
    pub rev: u64,
    /// 아직 줄이 끝나지 않은 채 흘러 들어오는 중인 값 (필드, 부분 문자열).
    pub live: Option<(String, String)>,
}

impl Card {
    pub fn field(&self, name: &str) -> &str {
        if let Some((f, v)) = &self.live {
            if f == name {
                return v;
            }
        }
        self.fields.get(name).map(|s| s.as_str()).unwrap_or("")
    }
}

/// 보관된 주제 하나: 화면에서는 내려갔지만 카드째로 남아 있어서 언제든 다시 꺼낼 수 있다.
#[derive(Clone, Debug)]
pub struct Shelved {
    pub topic: Topic,
    pub cards: Vec<Card>,
}

#[derive(Default)]
pub struct Board {
    pub groups: Vec<Group>,
    pub topics: Vec<Topic>,
    pub cards: Vec<Card>,
    /// 보관함. 끝났거나 오래 손대지 않은 주제가 여기로 내려간다.
    pub shelf: Vec<Shelved>,
    pub say: String,
    pub focus: Option<String>,
    /// 한도 초과로 잘린 것 등, 다음 턴에 모델에게 알려줄 내용.
    pub notes: Vec<String>,
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn clip(s: &str, limit: usize) -> String {
    if chars(s) <= limit {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(limit.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && chars(id) <= 24 && id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

impl Board {
    pub fn card(&self, id: &str) -> Option<&Card> {
        self.cards.iter().find(|c| c.id == id)
    }
    fn card_mut(&mut self, id: &str) -> Result<&mut Card, String> {
        self.cards.iter_mut().find(|c| c.id == id).ok_or_else(|| format!("카드 '{id}' 없음"))
    }

    /// 조작 하나를 검사하고 반영한다. 거부되면 Err(이유).
    pub fn apply(&mut self, op: Op, by: Author) -> Result<(), String> {
        match op {
            Op::Topic { id, name } => {
                if !valid_id(&id) {
                    return Err(format!("주제 ID '{id}' 형식 오류"));
                }
                if self.shelf.iter().any(|s| s.topic.id == id) {
                    return Err(format!("주제 '{id}' 는 보관되어 있음. 다시 쓰려면 restore {id} 로 꺼낸다"));
                }
                let name = clip(if name.is_empty() { &id } else { &name }, 24);
                match self.topics.iter_mut().find(|t| t.id == id) {
                    Some(t) => t.name = name,
                    None => self.topics.push(Topic { id, name, group: None }),
                }
            }
            Op::Group { id, name } => {
                if !valid_id(&id) {
                    return Err(format!("분류 ID '{id}' 형식 오류"));
                }
                if self.topics.iter().any(|t| t.id == id) || self.card(&id).is_some() {
                    return Err(format!("'{id}' 는 이미 주제나 카드의 ID 로 쓰이고 있음"));
                }
                let name = clip(if name.is_empty() { &id } else { &name }, 16);
                match self.groups.iter_mut().find(|g| g.id == id) {
                    Some(g) => g.name = name,
                    None => {
                        if self.groups.len() >= 8 {
                            return Err("분류는 8개까지. 더 넓게 묶을 것".into());
                        }
                        self.groups.push(Group { id, name });
                    }
                }
            }
            Op::Mv { card, topic } => {
                if !self.topics.iter().any(|t| t.id == topic) {
                    return Err(format!("주제 '{topic}' 없음"));
                }
                let c = self.card_mut(&card)?;
                c.topic = topic;
                c.grid = None; // 새 주제 구역에서 자리를 다시 잡는다
                c.pinned = false;
                c.rev += 1;
            }
            Op::Put { topic, group } => {
                if !self.groups.iter().any(|g| g.id == group) {
                    return Err(format!("분류 '{group}' 없음 — 먼저 group 으로 만들 것"));
                }
                let t = self.topics.iter_mut().find(|t| t.id == topic).ok_or_else(|| format!("주제 '{topic}' 없음"))?;
                t.group = Some(group);
            }
            Op::Card { id, kind, topic, size, title } => {
                if self.shelf.iter().any(|s| s.cards.iter().any(|c| c.id == id)) {
                    return Err(format!("카드 ID '{id}' 는 보관된 주제의 카드가 쓰고 있음. 다른 ID 를 쓴다"));
                }
                if !valid_id(&id) {
                    return Err(format!("카드 ID '{id}' 형식 오류"));
                }
                if self.card(&id).is_some() {
                    return Err(format!("카드 '{id}' 는 이미 있음 — 새로 만들지 말고 set/add/upd 로 고칠 것"));
                }
                let k = Kind::parse(&kind).ok_or_else(|| format!("부품 종류 '{kind}' 없음"))?;
                if !self.topics.iter().any(|t| t.id == topic) {
                    return Err(format!("주제 '{topic}' 없음 — 먼저 topic 으로 만들 것"));
                }
                let (w, h) = size.unwrap_or(k.default_size());
                if chars(&title) > 40 {
                    self.notes.push(format!("{id}.title 40자 초과로 잘림"));
                }
                self.cards.push(Card {
                    id,
                    kind: k,
                    topic,
                    title: clip(&title, 40),
                    fields: BTreeMap::new(),
                    items: Vec::new(),
                    w: w.clamp(1, MAX_W),
                    h: h.clamp(1, MAX_H),
                    grid: None,
                    pinned: false,
                    rev: 1,
                    live: None,
                });
            }
            Op::Set { id, field, value } => {
                let mut note = None;
                let c = self.card_mut(&id)?;
                let limit = c
                    .kind
                    .field_limit(&field)
                    .ok_or_else(|| format!("{} 에는 '{field}' 필드 없음", c.kind.name()))?;
                match (c.kind, field.as_str()) {
                    (Kind::Status, "state") if !["ok", "run", "wait", "fail", "idle"].contains(&value.as_str()) => {
                        return Err(format!("status.state 는 ok|run|wait|fail|idle 중 하나 ('{value}')"));
                    }
                    (Kind::Progress, "pct") if value.parse::<u8>().map(|v| v > 100).unwrap_or(true) => {
                        return Err(format!("progress.pct 는 0~100 정수 ('{value}')"));
                    }
                    _ => {}
                }
                if chars(&value) > limit {
                    note = Some(format!("{id}.{field} {limit}자 초과로 잘림 — 더 짧게 쓰거나 다른 부품을 쓸 것"));
                }
                let v = clip(&value, limit);
                if field == "title" {
                    c.title = v;
                } else {
                    c.fields.insert(field, v);
                }
                c.live = None;
                c.rev += 1;
                self.notes.extend(note);
            }
            Op::Add { id, item, cells } => self.put_item(&id, &item, cells, false)?,
            Op::Upd { id, item, cells } => self.put_item(&id, &item, cells, true)?,
            Op::Del { id, item: Some(item) } => {
                let c = self.card_mut(&id)?;
                let n = c.items.len();
                c.items.retain(|(i, _)| *i != item);
                if c.items.len() == n {
                    return Err(format!("{id} 에 항목 '{item}' 없음"));
                }
                c.rev += 1;
            }
            Op::Del { id, item: None } => {
                let n = self.cards.len();
                self.cards.retain(|c| c.id != id);
                if self.cards.len() == n {
                    // 카드가 아니면 주제 삭제로 본다 (비어 있을 때만).
                    if self.topics.iter().any(|t| t.id == id) {
                        if self.cards.iter().any(|c| c.topic == id) {
                            return Err(format!("주제 '{id}' 에 카드가 남아 있음"));
                        }
                        self.topics.retain(|t| t.id != id);
                    } else if self.groups.iter().any(|g| g.id == id) {
                        // 분류를 지우면 그 안의 주제는 미분류로 돌아간다.
                        self.groups.retain(|g| g.id != id);
                        for t in self.topics.iter_mut().filter(|t| t.group.as_deref() == Some(id.as_str())) {
                            t.group = None;
                        }
                    } else {
                        return Err(format!("'{id}' 없음"));
                    }
                }
                if self.focus.as_deref() == Some(id.as_str()) {
                    self.focus = None;
                }
            }
            Op::Size { id, w, h } => {
                let c = self.card_mut(&id)?;
                if by == Author::Ai && c.pinned {
                    return Err(format!("{id} 는 사용자가 고정함 — 크기 변경 불가"));
                }
                c.w = w.clamp(1, MAX_W);
                c.h = h.clamp(1, MAX_H);
                if by == Author::User {
                    c.pinned = true;
                }
                c.rev += 1;
            }
            Op::Move { id, col, row } => {
                let c = self.card_mut(&id)?;
                if by == Author::Ai && c.pinned {
                    return Err(format!("{id} 는 사용자가 고정함 — 이동 불가"));
                }
                c.grid = Some((col.max(0), row.max(0)));
                if by == Author::User {
                    c.pinned = true;
                }
            }
            Op::Pin { id, on } => {
                if by == Author::Ai {
                    return Err("pin 은 사용자만 가능".into());
                }
                let c = self.card_mut(&id)?;
                c.pinned = on;
                if !on {
                    c.grid = None;
                }
            }
            Op::Focus { id } => {
                self.card_mut(&id)?;
                self.focus = Some(id);
            }
            Op::Archive { topic } => {
                let at = self.topics.iter().position(|t| t.id == topic).ok_or_else(|| format!("주제 '{topic}' 없음"))?;
                let t = self.topics.remove(at);
                let (gone, kept): (Vec<Card>, Vec<Card>) = std::mem::take(&mut self.cards).into_iter().partition(|c| c.topic == topic);
                self.cards = kept;
                if self.focus.as_ref().map(|f| gone.iter().any(|c| &c.id == f)).unwrap_or(false) {
                    self.focus = None;
                }
                self.shelf.push(Shelved { topic: t, cards: gone });
            }
            Op::Restore { topic } => {
                let at = self.shelf.iter().position(|s| s.topic.id == topic).ok_or_else(|| format!("보관함에 주제 '{topic}' 없음"))?;
                let mut s = self.shelf.remove(at);
                // 보관해 둔 사이에 분류가 없어졌으면 미분류로 꺼낸다.
                if s.topic.group.as_ref().map(|g| !self.groups.iter().any(|x| &x.id == g)).unwrap_or(false) {
                    s.topic.group = None;
                }
                self.topics.push(s.topic);
                for mut c in s.cards {
                    c.rev += 1;
                    self.cards.push(c);
                }
            }
            Op::Say { text } => {
                if chars(&text) > 80 {
                    self.notes.push("say 80자 초과로 잘림".into());
                }
                self.say = clip(&text, 80);
            }
        }
        Ok(())
    }

    fn put_item(&mut self, id: &str, item: &str, cells: Vec<String>, update: bool) -> Result<(), String> {
        let mut note = None;
        let c = self.card_mut(id)?;
        if !c.kind.has_items() {
            return Err(format!("{} 에는 항목을 넣을 수 없음", c.kind.name()));
        }
        if !valid_id(item) {
            return Err(format!("항목 ID '{item}' 형식 오류"));
        }
        if cells.is_empty() || cells.len() > MAX_CELLS {
            return Err(format!("항목의 칸 수는 1~{MAX_CELLS}"));
        }
        let states: &[&str] = match c.kind {
            Kind::Checklist => &["done", "todo"],
            Kind::Progress => &["done", "run", "todo", "fail"],
            _ => &[],
        };
        if !states.is_empty() && !states.contains(&cells[0].as_str()) {
            return Err(format!("{} 항목의 첫 칸은 {} 중 하나 ('{}')", c.kind.name(), states.join("|"), cells[0]));
        }
        if c.kind == Kind::Chart && cells.get(1).and_then(|v| v.replace(',', "").parse::<f64>().ok()).is_none() {
            return Err("chart 항목은 '이름 | 숫자'".into());
        }
        if cells.iter().any(|x| chars(x) > CELL_LIMIT) {
            note = Some(format!("{id}.{item} 칸 {CELL_LIMIT}자 초과로 잘림"));
        }
        let cells: Vec<String> = cells.iter().map(|x| clip(x, CELL_LIMIT)).collect();
        match c.items.iter_mut().find(|(i, _)| i == item) {
            Some(slot) => slot.1 = cells, // add 가 기존 ID 에 오면 덮어쓴다 (중복 생성 방지)
            None if update => return Err(format!("{id} 에 항목 '{item}' 없음 — add 로 추가할 것")),
            None => {
                if c.items.len() >= MAX_ITEMS {
                    return Err(format!("{id} 항목 수 한도({MAX_ITEMS}) 초과"));
                }
                c.items.push((item.to_string(), cells));
            }
        }
        c.rev += 1;
        self.notes.extend(note);
        Ok(())
    }
}

#[cfg(test)]
mod shelf_tests {
    use super::*;

    #[test]
    fn archive_and_restore_keep_cards() {
        let mut b = Board::default();
        let run = |b: &mut Board, line: &str| b.apply(crate::lang::parse(line).unwrap().unwrap(), Author::Ai);
        run(&mut b, "topic trip 제주 여행").unwrap();
        run(&mut b, "card k1 note trip 메모").unwrap();
        run(&mut b, "set k1 text 숙소는 애월").unwrap();
        run(&mut b, "archive trip").unwrap();
        assert!(b.topics.is_empty() && b.cards.is_empty() && b.shelf.len() == 1);
        // 보관된 것과 같은 ID 로 새로 만들 수 없다 (꺼낼 때 부딪히지 않게).
        assert!(run(&mut b, "topic trip 다른 여행").is_err());
        run(&mut b, "topic t2 다른 주제").unwrap();
        assert!(run(&mut b, "card k1 note t2 겹침").is_err());
        run(&mut b, "restore trip").unwrap();
        assert_eq!(b.card("k1").unwrap().field("text"), "숙소는 애월");
        assert!(b.shelf.is_empty());
    }
}
