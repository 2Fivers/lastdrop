# Lastdrop

How much AI is left in the tank, one click away.

Lastdrop is a Windows 11 tray app that shows your remaining usage across AI services —
Claude, Codex, GitHub Copilot, Meshy and more — without opening each service's usage page. Click the tray
icon and you see what is left, when it resets, what you used recently, and whether your
current pace will last until the next reset.

## Goals

- **One glance.** A native Windows 11 flyout next to the volume and network flyouts.
- **Light on memory.** It runs all day, so it should cost next to nothing while hidden.
- **Your choice of what to show.** Pick the services, the meters and the widgets you care about.
- **Real statistics.** History, burn rate, pace and a forecast of when you run out.
- **Easy to extend.** Simple balance endpoints are described in a JSON file; no compiler needed.
- **Your data stays yours.** Everything is read and stored locally. Nothing is sent anywhere
  except to the service whose usage is being read.

## Status

In development, not yet distributed. Built in Rust with Slint's software renderer, the stack
the spike picked by measuring idle memory and flyout latency
([ADR 0001](docs/adr/0001-rust-and-slint-software-renderer.md)).

The first useful build is done: tray icon, taskbar readout, flyout, hover card, settings and
autostart, reading Claude, Codex, GitHub Copilot, Cursor, Google Antigravity, OpenCode Go, Grok and
Meshy, with notifications when a limit
reaches 80 % or 95 %, is used up or resets. It speaks English and Swedish, following Windows'
display language unless one is picked in settings. Claude Code and Codex can have several
accounts, one per config folder. Services with a simple balance or usage API are described in a
JSON file (DeepSeek, ElevenLabs and OpenRouter come with it). A usage history window charts each
limit over the last 7, 30 or 90 days, the use per day and the hours of the week with the most use.

## Plan

See [docs/ROADMAP.md](docs/ROADMAP.md) for providers, features, milestones and what is built,
and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the app is put together.

## License

Proprietary. Copyright © 2026 Adinosaur AB, all rights reserved; distribution is not
permitted. See [LICENSE](LICENSE).
