# ALI

**Adaptive Live Interface** · [한국어](README.ko.md)

> A screen full of text is the worst way to use AI.

Humanity built an extraordinary engine and bolted it to a wooden cart. The engine is AI. The cart is a chat window full of text.

Every answer comes back as a scroll of paragraphs. Most of it goes unread. The chat window, a tool made for people typing to each other, became the place where we work with a machine that can do far more than type.

People have already built many excellent project and collaboration tools: Jira, monday.com, Asana, Notion, Trello. Yet in front of AI, all of us went back to plain text. And those tools are too complex, with features spread too wide, to be the place where you talk to an AI.

What we want is one place. Every conversation and every topic lives there, shown as the information you actually need, shaped so it reads at a glance, in an interface that adapts as you speak. No tsunami of text.

ALI is that place. You say what you need, and the AI arranges the screen for you: a schedule as a timeline, a trip as a route, a choice as a card you tap.

Nothing like it existed, so we built it. We hope it makes working with AI less tiring and more productive for everyone.

![ALI by FIDENIT: a wall of text gives way to a live board. One request plans a workshop; the route, schedule, budget and checklist appear, and the one decision that needs you comes to the left.](docs/images/hero.gif)

## What is different

**The screen is the answer.** The AI does not write to you. It edits a board of cards, one line at a time, and you watch the board change. Ask about the same thing again and the same card updates. The board shows where things stand now. It is not a transcript.

![Asking for a to-do list, then reporting progress: the same cards update](docs/images/todo.gif)

**Work comes to you.** When the AI needs a decision or a value from you, that one card moves to the "my turn" slot beside the conversation, and the board slides to the part of the work it belongs to. Answer it and the next one arrives. You do not go looking for what is waiting on you.

![The AI compares options, then brings the question it needs answered to the "my turn" slot](docs/images/myturn.gif)

**One board for everything.** No chat rooms and no threads. Topics sit on a single canvas, grouped into areas the AI creates, renames and merges as your work grows. Zoom out to see everything, zoom in to one card. Finished work is shelved and comes back when you mention it.

![Zooming out from one topic to the whole board, grouped by the AI](docs/images/canvas.gif)

**It draws.** Routes with real distances and travel times, line, bar and donut charts, flows and layouts, pictures. The AI picks the form that reads fastest.

![Asking for a drive: the AI finds the places, reads the travel time and draws the route](docs/images/route.gif)

![A trend line, a budget split and a release flow drawn on request](docs/images/charts.gif)

**It remembers.** Facts worth keeping are picked out after every turn. Old conversations related to what you just said are found and brought along. Everything is stored as plain Markdown in a folder you can open in Obsidian.

**It does the work.** It runs commands, reads and edits files, searches the web and drives a browser on the board. Longer jobs are written up as a work order and handed to workers that run in parallel while you keep talking.

**You can talk to it.** Speak and listen instead of typing. Interrupt it while it is speaking.

## Try it

You can look around without anything: with no key, ALI opens in a demo that plays prepared scenes and says so on screen. To really use it you need a [Claude API key](https://console.anthropic.com/settings/keys). Usage is billed to your own account. The key can also come from the `ANTHROPIC_API_KEY` environment variable.

**Or use Claude Code as the engine.** If [Claude Code](https://claude.com/claude-code) is installed, choose it in the same window and ALI hands each turn to it instead of calling the API. Your Claude Code login, `CLAUDE.md`, skills, hooks, MCP servers and permission settings all apply, and no API key is needed. Files, commands and the web go through Claude Code's tools, while ALI's own tools (route maps, memory notes, reading the board, workers) are offered to it through a small local MCP server inside ALI. Workers and memory keeping also run through Claude Code. A turn costs more tokens than with the API key, because Claude Code brings its own instructions. By default it does only what your Claude Code settings allow; the same window lets you allow file edits, or everything. Extra options for Claude Code, such as `--model sonnet`, go in `claude-args.txt` in the program folder.

A second key is optional: a [TypeSafe AI](https://typesafe.ai) Jev key. Jev is a small, fast model that only makes decisions. With it, ALI picks the topic before answering, puts important cards first, tidies overlapping topics, and recalls only the past conversations that matter. Without it, ALI works the same. Both keys can be entered or changed any time from **Keys** at the bottom left (or `TYPESAFE_API_KEY`).

**Windows.** Download the zip from [Releases](https://github.com/fidenit/ALI/releases), unpack it and run `ali.exe`. There is no installer. On first start a window asks for your key; you can skip it and stay in the demo.

**Mac.** Build it yourself for now: `./run.sh pack` creates `portable/ALI/ALI.app`. See [DEVELOPING](docs/DEVELOPING.md).

Keys, the board and the memory vault live in the program's own folder. Move the folder and everything moves with it.

## Know before you run

- **The AI acts without asking.** It runs commands and changes files on your computer on its own. ALI was built to be used that way. Run it knowing this, on a machine where that is acceptable.
- **It costs money as you use it.** By default it uses the most capable Claude model.
- **It is early.** One person uses it daily. Korean and English, switched in the corner. See [what is next](docs/NEXT.md).
- Distances and times on route cards are read from Kakao Map, so routes work in South Korea.

## For developers

Rust, [egui](https://github.com/emilk/egui), and the Claude API called directly over HTTP. No web front end. The model's output is a small line-based language for editing the board, applied as each line arrives; the board, written back in the same language, is what the model sees on the next turn.

Build steps, the layout of the code and how to add a new kind of card are in [docs/DEVELOPING.md](docs/DEVELOPING.md) (Korean).

Most of the code was written together with AI (Claude). The idea and the design decisions are human.

## License

[Apache License 2.0](LICENSE). Keep the [NOTICE](NOTICE) file when you redistribute. The name "ALI" may not be used as the name of a derived product.

---

**FIDENIT** · for the world.
