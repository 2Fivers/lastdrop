# Lastdrop

How much AI is left in the tank, one click away.

Lastdrop is a Windows 11 tray app that shows your remaining usage across AI services —
Claude, Codex, Meshy and more — without opening each service's usage page. Click the tray
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

Early development. The stack is being chosen by measuring idle memory and flyout latency
of the candidates on a real machine before the first line of the app is written.

## License

[MIT](LICENSE)
