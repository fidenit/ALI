# ALI — Adaptive Live Interface

CLI(명령 줄), GUI(그림) 다음의 방식. AI와 채팅창으로 대화하지 않는다.
AI가 답을 글로 쌓는 대신 화면을 직접 구성하고 고친다. 사용자는 긴 글을 읽지 않고 한눈에 본다.

- **화면이 유일한 출력.** 모델은 문장 대신 "화면 수정 명령"을 한 줄씩 내고, 줄이 끝나는 대로 화면에 반영된다.
- **쌓지 않고 고친다.** 같은 주제를 다시 다루면 새 글이 붙는 게 아니라 그 카드가 바뀐다. 화면은 대화 기록이 아니라 현재 상태다.
- **하나의 넓은 판.** 분류 → 주제 → 카드가 한 판 위에 있고, 스크롤로 확대·축소하며 오간다. 채팅방을 나누지 않는다.
- **할 일이 나를 찾아온다.** 사용자의 답이 필요한 것은 대화창 위의 "내 차례" 자리로 하나씩 오고, 판은 그 일이 속한 구역을 바로 옆에 붙여 보여 준다.
- **직접 일한다.** 명령 실행, 파일 읽기·쓰기, 웹 검색, 화면 안의 브라우저 조작을 묻지 않고 한다.
- **기억은 옵시디언 보관함.** 모든 턴이 `vault/기록/날짜.md`에 남고, 오래 둘 사실은 AI가 `vault/기억/`에 쓴다.

Rust로 만든 네이티브 프로그램이고, Claude API를 HTTP로 직접 부른다.

> **알아 둘 것.** 이 프로그램의 AI는 사용자의 컴퓨터에서 명령 실행과 파일 읽기·쓰기를 **묻지 않고** 한다. 그렇게 쓰도록 만든 프로그램이다. 중요한 자료가 있는 컴퓨터에서는 이 점을 알고 쓴다. 사용한 만큼 Claude API 요금이 본인 계정에 청구된다.

## 받아서 쓰기 (Windows)

[Releases](https://github.com/fidenit/ALI/releases)에서 `ALI-…-windows-x64.zip`을 받아 풀고 `ali.exe`를 실행한다. 설치는 없다. 처음 실행하면 Claude API 키를 넣는 칸이 나온다. 자세한 것은 묶음 안의 `읽어보기.txt`.

## Mac

아직 받아 쓰는 묶음은 없다. 직접 빌드한다 (아래). `./run.sh pack`이 `portable/ALI/ALI.app`을 만든다. 키 파일과 `data/`, `vault/`는 `ALI.app` 옆에 둔다. 서명은 임시 서명(ad-hoc)뿐이라, 다른 Mac으로 옮기면 처음 한 번은 Finder에서 오른쪽 클릭 → 열기로 연다. 인터넷에서 받은 경우 `xattr -dr com.apple.quarantine ALI.app`이 필요할 수 있다.

## 직접 빌드해서 실행

Windows:

```powershell
.\run.ps1          # 빌드해서 실행
.\run.ps1 test     # 테스트
.\run.ps1 pack     # 들고 다닐 수 있는 폴더 portable\ALI 만들기
```

Mac (Rust는 [rustup](https://rustup.rs)으로 설치, Xcode 명령줄 도구 필요):

```sh
./run.sh            # 빌드해서 실행
./run.sh test       # 테스트
./run.sh pack       # portable/ALI/ALI.app 만들기
```

Windows 빌드에는 Rust(`stable-x86_64-pc-windows-gnu`)와 MinGW(WinLibs)가 필요하다. 실행 파일 옆에 `WebView2Loader.dll`이 있어야 뜬다(`run.ps1`이 복사한다).

## 키

프로그램 폴더(실행 파일이 있는 폴더)에 텍스트 파일로 둔다. 처음 실행하면 왼쪽 구역에서 입력할 수도 있다.

| 파일 | 내용 | 없으면 |
|---|---|---|
| `key.txt` | Claude API 키 | 시작할 수 없음 (필수) |
| `jev.txt` | TypeSafe Jev 키 | 판단 모델 없이 동작 |
| `eleven.txt` | ElevenLabs 키 | OS 기본 음성으로 읽고, 마이크 대화는 못 씀 |
| `voice.txt` | ElevenLabs 목소리 ID | 기본 목소리 |

키 파일과 `data/`, `vault/`는 저장소에 올라가지 않는다.

## 부품 더하기

화면에 놓이는 카드의 종류(부품)는 `src/core.rs`의 `PARTS` 한 곳에 등록되어 있다. 부품 하나에 관한 것(이름, 기본 크기, 받는 필드와 글자 한도, 항목 형식, 무엇에 쓰는지)이 모두 거기 있고, AI에게 주는 부품 표도 이 목록에서 만들어진다. 지시문(`prompt.md`)의 표를 따로 고치지 않는다.

새 부품을 더하려면 세 군데를 고친다.

1. `src/core.rs`의 `Kind`에 종류를 하나 더한다.
2. 같은 파일의 `PARTS`에 등록한다. `purpose`와 필드 설명은 AI가 부품을 고를 때 읽는 글이다.
3. `src/ui.rs`의 `draw_body`에 그리는 법을 더한다 (컴파일러가 빠진 종류를 알려 준다).

항목에 지켜야 할 형식이 있으면 `core.rs`의 `put_item`에 검사를 더한다. 사용자의 답을 받는 부품이면 `Kind::asks`에 넣는다. 그러면 "내 차례" 자리로 온다.

## 구조

| 파일 | 역할 |
|---|---|
| `src/core.rs` | 보드(분류, 주제, 카드)와 조작, 타입 검사 |
| `src/lang.rs` | 화면 수정 언어: 해석, 직렬화, 스트림을 줄 단위로 끊기 |
| `src/prompt.md` | 모델 지시문 |
| `src/agent.rs` | 한 턴의 작업 루프 (모델 호출 ↔ 도구 실행) |
| `src/tools.rs` | 도구: 명령 실행, 파일, 기억 노트, 브라우저 조작, 웹 |
| `src/decide.rs`, `src/jev.rs` | 메인 모델 앞뒤의 작은 결정 (Jev) |
| `src/ui.rs` | 판 그리기: 배치, 확대·축소, 전환, 부품, 직접 조작 |
| `src/chat.rs` | 왼쪽 대화 구역 |
| `src/vault.rs` | 옵시디언 보관함 (기록, 기억) |
| `src/tts.rs`, `src/eleven.rs`, `src/audio.rs` | 말하기, 받아쓰기, 스피커, 마이크 |
| `src/sys.rs` | 프로그램 폴더, 시각, 창 상단바, 옵시디언 열기 |

## 수정 언어 예

```
say 두 후보를 비교했어요. 골라 주세요.
topic ws 팀 워크숍
card k1 table ws 5x2 장소 비교
set k1 cols 장소 | 이동 시간 | 1인 비용
add k1 r1 강릉 | 2시간 40분 | 18만원
card k2 decision ws 3x2 장소 결정
set k2 text 어디로 할까요?
add k2 o1 강릉 | 멀지만 시설이 좋음
```

## 주의

AI가 사용자의 컴퓨터에서 명령을 **확인 없이** 실행한다. 본인 PC에서 본인 책임으로 쓰는 것을 전제로 만들었다.

## 라이선스

[Apache License 2.0](LICENSE). 가져다 쓰거나 고쳐서 배포할 때는 [NOTICE](NOTICE)를 함께 둔다. "ALI"라는 이름은 파생 제품의 이름으로 쓸 수 없다.

코드의 대부분은 AI(Claude)와 함께 작성했다. 구상과 설계의 결정은 사람이 했다.
