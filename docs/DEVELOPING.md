# 개발자용 안내

빌드, 키, 부품을 더하는 법, 코드의 구조. 프로그램이 무엇인지는 [README](../README.ko.md)에 있다.

## 빌드해서 실행

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
