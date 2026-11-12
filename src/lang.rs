//! 화면 수정 언어. 한 줄이 조작 하나이고, 줄이 끝나는 순간 적용된다.
//! 보드를 모델에게 보여줄 때도 같은 언어로 직렬화한다.

use crate::core::{Board, Op};

fn split1(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    }
}

fn need<'a>(s: &'a str, what: &str) -> Result<&'a str, String> {
    if s.is_empty() { Err(format!("{what} 누락")) } else { Ok(s) }
}

fn parse_size(s: &str) -> Option<(u8, u8)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn cells(rest: &str) -> Vec<String> {
    rest.split('|').map(|c| c.trim().to_string()).collect()
}

/// 한 줄을 조작으로 해석한다. 빈 줄·주석·코드 울타리는 Ok(None).
pub fn parse(line: &str) -> Result<Option<Op>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with("```") {
        return Ok(None);
    }
    let (cmd, rest) = split1(line);
    let op = match cmd {
        "topic" => {
            let (id, name) = split1(rest);
            Op::Topic { id: need(id, "주제 ID")?.into(), name: name.into() }
        }
        "group" => {
            let (id, name) = split1(rest);
            Op::Group { id: need(id, "분류 ID")?.into(), name: name.into() }
        }
        "put" => {
            let (topic, group) = split1(rest);
            Op::Put { topic: need(topic, "put <주제ID> <분류ID>")?.into(), group: need(group, "put <주제ID> <분류ID>")?.into() }
        }
        "card" => {
            let (id, rest) = split1(rest);
            let (kind, rest) = split1(rest);
            let (topic, rest) = split1(rest);
            need(topic, "card <id> <종류> <주제>")?;
            // 크기는 생략 가능
            let (maybe, after) = split1(rest);
            let (size, title) = match parse_size(maybe) {
                Some(s) => (Some(s), after),
                None => (None, rest),
            };
            Op::Card { id: id.into(), kind: kind.into(), topic: topic.into(), size, title: title.into() }
        }
        "set" => {
            let (id, rest) = split1(rest);
            let (field, value) = split1(rest);
            Op::Set { id: id.into(), field: need(field, "set <id> <필드>")?.into(), value: value.into() }
        }
        "add" | "upd" => {
            let (id, rest) = split1(rest);
            let (item, rest) = split1(rest);
            need(item, "항목 ID")?;
            let c = cells(need(rest, "항목 내용")?);
            if cmd == "add" {
                Op::Add { id: id.into(), item: item.into(), cells: c }
            } else {
                Op::Upd { id: id.into(), item: item.into(), cells: c }
            }
        }
        "del" => {
            let (id, item) = split1(rest);
            Op::Del { id: need(id, "ID")?.into(), item: if item.is_empty() { None } else { Some(item.into()) } }
        }
        "size" => {
            let (id, s) = split1(rest);
            let (w, h) = parse_size(s).ok_or("size <id> <가로>x<세로>")?;
            Op::Size { id: id.into(), w, h }
        }
        "move" => {
            let (id, rest) = split1(rest);
            let (c, r) = split1(rest);
            match (c.parse(), r.parse()) {
                (Ok(col), Ok(row)) => Op::Move { id: id.into(), col, row },
                _ => return Err("move <id> <열> <행>".into()),
            }
        }
        "pin" | "unpin" => Op::Pin { id: need(rest, "ID")?.into(), on: cmd == "pin" },
        "focus" => Op::Focus { id: need(rest, "ID")?.into() },
        "say" => Op::Say { text: rest.into() },
        // 소리 내어 읽는 문장. 보드를 바꾸지 않으므로 조작이 아니다 (음성 재생 쪽에서 따로 처리한다).
        "tell" => return Ok(None),
        _ => return Err(format!("모르는 명령 '{cmd}'")),
    };
    Ok(Some(op))
}

/// 보드 전체를 수정 언어로 적는다 (모델 입력용). 이 줄들을 빈 보드에 적용하면 같은 보드가 된다.
pub fn serialize(b: &Board) -> String {
    let mut s = String::new();
    for g in &b.groups {
        s += &format!("group {} {}\n", g.id, g.name);
    }
    for t in &b.topics {
        s += &format!("topic {} {}\n", t.id, t.name);
        if let Some(g) = &t.group {
            s += &format!("put {} {}\n", t.id, g);
        }
    }
    for c in &b.cards {
        s += &format!("card {} {} {} {}x{} {}\n", c.id, c.kind.name(), c.topic, c.w, c.h, c.title);
        for (f, v) in &c.fields {
            s += &format!("set {} {} {}\n", c.id, f, v);
        }
        for (i, cells) in &c.items {
            s += &format!("add {} {} {}\n", c.id, i, cells.join(" | "));
        }
        if let Some((col, row)) = c.grid {
            s += &format!("move {} {} {}\n", c.id, col, row);
        }
        if c.pinned {
            s += &format!("pin {}\n", c.id);
        }
    }
    s
}

pub enum Chunk {
    /// 완성된 한 줄
    Line(String),
    /// 아직 끝나지 않은 현재 줄 (지금까지 받은 부분)
    Partial(String),
}

/// 토큰 스트림을 줄 단위로 끊는다.
#[derive(Default)]
pub struct LineStream {
    buf: String,
}

impl LineStream {
    pub fn feed(&mut self, delta: &str) -> Vec<Chunk> {
        self.buf.push_str(delta);
        let mut out = Vec::new();
        while let Some(i) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=i).collect();
            out.push(Chunk::Line(line.trim_end().to_string()));
        }
        if !self.buf.is_empty() {
            out.push(Chunk::Partial(self.buf.clone()));
        }
        out
    }
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        if rest.trim().is_empty() { None } else { Some(rest) }
    }
}

/// 진행 중인 줄이 긴 글을 쓰는 중이면 (카드 ID, 필드, 지금까지의 값).
/// `say` 는 카드 ID 가 빈 문자열.
pub fn partial_text(partial: &str) -> Option<(String, String, String)> {
    let (cmd, rest) = split1(partial);
    match cmd {
        "say" => Some((String::new(), "say".into(), rest.to_string())),
        "set" => {
            let (id, rest) = split1(rest);
            let (field, value) = split1(rest);
            if field == "text" && !value.is_empty() {
                Some((id.to_string(), field.to_string(), value.to_string()))
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Author, Board};

    fn run(b: &mut Board, src: &str, by: Author) -> Vec<String> {
        let mut errs = Vec::new();
        for l in src.lines() {
            match parse(l) {
                Ok(Some(op)) => {
                    if let Err(e) = b.apply(op, by) {
                        errs.push(e);
                    }
                }
                Ok(None) => {}
                Err(e) => errs.push(e),
            }
        }
        errs
    }

    #[test]
    fn roundtrip() {
        let mut b = Board::default();
        let errs = run(
            &mut b,
            "topic t1 여행\ncard k1 table t1 4x3 항공편 비교\nset k1 cols 항공사 | 가격\nadd k1 r1 A | 100\nadd k1 r2 B | 120\ncard k2 status t1 준비\nset k2 state run\nsay 정리했어요",
            Author::Ai,
        );
        assert!(errs.is_empty(), "{errs:?}");
        let text = serialize(&b);
        let mut b2 = Board::default();
        assert!(run(&mut b2, &text, Author::User).is_empty());
        assert_eq!(serialize(&b2), text);
        assert_eq!(b.card("k2").unwrap().w, 2); // 기본 크기
    }

    #[test]
    fn rejects() {
        let mut b = Board::default();
        let errs = run(
            &mut b,
            "card k1 note t9 제목\ntopic t1 a\ncard k1 note t1 제목\ncard k1 note t1 또\nset k1 state ok\nadd k1 i1 x\nfoo bar\nset k9 text x",
            Author::Ai,
        );
        assert_eq!(errs.len(), 6, "{errs:?}");
        assert_eq!(b.cards.len(), 1);
    }

    #[test]
    fn user_pin_wins() {
        let mut b = Board::default();
        run(&mut b, "topic t a\ncard k note t x", Author::Ai);
        assert!(run(&mut b, "move k 3 2\nsize k 5x4", Author::User).is_empty());
        assert_eq!(run(&mut b, "move k 0 0\nsize k 1x1", Author::Ai).len(), 2);
        let c = b.card("k").unwrap();
        assert_eq!((c.grid, c.w, c.h, c.pinned), (Some((3, 2)), 5, 4, true));
    }

    #[test]
    fn limits_clip_and_note() {
        let mut b = Board::default();
        let long = "가".repeat(300);
        run(&mut b, &format!("topic t a\ncard k note t x\nset k text {long}"), Author::Ai);
        assert_eq!(b.card("k").unwrap().field("text").chars().count(), 240);
        assert_eq!(b.notes.len(), 1);
    }

    #[test]
    fn streaming() {
        let mut s = LineStream::default();
        let mut lines = Vec::new();
        let mut last_partial = String::new();
        for d in ["topic t 가\nset k te", "xt 안녕", "하세요\nsay 끝"] {
            for c in s.feed(d) {
                match c {
                    Chunk::Line(l) => lines.push(l),
                    Chunk::Partial(p) => last_partial = p,
                }
            }
        }
        assert_eq!(lines, vec!["topic t 가", "set k text 안녕하세요"]);
        assert_eq!(partial_text(&last_partial), Some((String::new(), "say".into(), "끝".into())));
        assert_eq!(s.finish().as_deref(), Some("say 끝"));
    }
}
