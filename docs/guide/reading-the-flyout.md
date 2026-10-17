# Reading the flyout

Click the tray icon or the taskbar readout to open the flyout. It has a header with one line about
how things stand, a card for each service and account, and a footer.

Every percentage in Lastdrop is the share **used**, never the share left. Something used never
shows as 0 %, and a limit shows 100 % only when the service itself says it is used up; until then
it stops at 99 %.

## The header

The line under the title names the most urgent thing, for example *Claude 5-hour runs out at
15:20* or *Copilot: sign in again*. When more than one thing needs attention it adds *· 2 more*.
When all is well it says *All limits last until their reset*.

The round arrow next to it refreshes every service (**F5** or **Ctrl+R**). A service that was read
less than its minimum interval ago, or that is waiting after an error, keeps its timer, so a
refresh can look like it did nothing for that service.

## A service card

The top line of a card has:

- the service's icon and name. With more than one account it also names the account, such as
  *Claude · Work* ([Accounts](accounts.md));
- the plan, when the service reports one;
- the age of the reading: *Just now*, *4 min ago*, *2 h ago*;
- a small chart of the last 14 days, today on the right: how much of the service's longest limit
  (the weekly one, when there is one) was used each day, or for a balance how much was spent.

Then one row per meter, or the balance.

### A meter row

A meter is a usage limit that resets: Claude's 5-hour session and weekly limits, Codex's windows,
Copilot's monthly premium requests and so on. Each row has:

- the meter's name and the percentage used;
- a bar. The fill is the share used. The small tick on the bar is the share of the window's time
  that has gone by. A fill that stops short of the tick means you are using it more slowly than
  time passes;
- on the right, when it resets: *Resets 14:56* within 12 hours, *Resets Mon 09:00* within the
  week, *Resets Sun 1 Nov* after that. Times and dates follow your regional format in Windows. A window that only starts when you
  next use it, like Claude's 5-hour session, says *Starts with your next use*. Some services give
  no reset time: *No reset time given*.

When a meter is in trouble, the right side and a line under the bar say so:

| What you see | What it means |
|---|---|
| ▲ *Out 16:00* and *Runs out 2 h before the reset at 18:00.* | At your recent pace this limit runs out before it resets. |
| ▲ *On course for 98% at the 18:00 reset.* | It was going to run out, and is now close to the edge. |
| ● *25 min left* and *Empty ≈ 15:45, 2 h 15 min before the 18:00 reset.* | It runs out within about half an hour. |
| ● *Nearly used up. Lasts until the reset at your recent pace.* | 90 % or more is used, but your pace gets you to the reset. |
| ● *Back 18:00* and *Used up since 14:10. Back in 3 h 50 min.* | The limit is used up until it resets. |

▲ uses the warning colour and ● the critical colour of your theme. A meter leaves a level only
with some margin, so it does not flicker between two states: one that turned red at 90 % stays red
until it is back to 87 %.

### A balance

A balance is an amount that goes down until you top it up, such as Meshy's credits or an
OpenRouter or DeepSeek balance. Instead of a bar it shows:

- the amount and its unit;
- how long it lasts at your recent use: *≈ 3 weeks left*. Or *More than 8 weeks left*, *No use in
  7 days*, or *Not enough history yet* in the first two days;
- how much of your last top-up is spent and the burn rate, *42% of last top-up used · 1.20 a day*,
  and the day it runs out, *until ≈ Mon 3 Nov*.

### Messages on a card

| Message | What it means |
|---|---|
| *Offline · showing 14:05 · next try 14:20* | Your PC has no connection. The numbers are the last ones read. |
| *Not answering · showing … · next try …* | The service did not answer, answered with an error, or asked Lastdrop to wait. |
| *No new reading · …* | The service answers, but its newest numbers are old. |
| *Sign-in expired. Lastdrop never signs in for you.* *Open sign-in…* | The tool Lastdrop reads through is signed out. *Open sign-in…* opens that tool's own sign-in in a terminal. |
| *API key missing or rejected.* *Enter key…* | A service read with an API key has none, or the key was refused. *Enter key…* opens the settings. |
| *Couldn't read …* *Retry* | The first reading failed, so there are no numbers to show yet. |
| *Paused · showing …* *Resume* | You paused updates from the tray menu. |

Old numbers are drawn with dashed, dimmed bars, and they get no warning colours: Lastdrop does not
warn about numbers it cannot vouch for. A reading counts as old after 15 minutes or two refresh
intervals, whichever is longer (60 minutes when refreshing by hand).
[Troubleshooting](troubleshooting.md) says what to do about each message.

### Using a card

- **Right-click** a card (or **Shift+F10**) for its menu: *Open usage page* (the service's own usage
  page in your browser), *Usage history…*, *Refresh* and *Show in taskbar readout*, which pins or
  unpins one of the card's meters.
- With the keyboard, **Up** and **Down** move between cards and **Enter** opens the usage page.

## The footer

The footer has a button for the usage history (**Ctrl+H**) and one for the settings. On the left it
says *Updates paused · Resume* while updates are paused, and *Lastdrop x.y.z is available ·
Install* when there is a new version ([Getting started](getting-started.md#updates)).

## Pace, forecast and burn rate

### Pace

Pace compares how much of a limit is used with how much of its time has gone. If 40 % of a weekly
limit is used two days into the week (29 % of the time), you are using it faster than time passes.
Within 3 percentage points either way counts as about the same.

Pace needs a reset time and a known window length. It is what the tick on the bar shows, and the
hover card puts it in words: *It is being used more slowly than time passes.* Pace alone never
turns a meter amber or red; only the forecast does.

### Forecast

The forecast takes your recent rate of use and carries it forward to the reset. "Recent" is a
tenth of the window, at least 30 minutes and at most 24 hours: the last 30 minutes for a 5-hour
window, about the last 17 hours for a weekly one, the last day for a monthly one.

From that rate Lastdrop works out what you will have used at the reset, and whether and when you
run out before it. The hover card shows it as *on course for 62% at reset*, *lasts to the reset* or
*runs out ≈ 15:20*. There is no forecast (*no forecast yet*) until there are two readings at least
15 minutes apart and the window has run for 15 minutes, and none for a meter without a reset time
or with old numbers.

### Burn rate

For a balance, the burn rate is how much it went down over the last 7 days, divided by 7. Top-ups
are left out. *≈ 3 weeks left* is the balance divided by that rate. It needs two days of history
before Lastdrop shows it. In the first week the 7 days are not full yet, so the rate reads low and
the time left reads long.

A balance turns amber when less than 7 days are left and red when less than 2 days are left.

## The usage history window

Open it with *Usage history…* in the tray menu, the history button in the flyout's footer
(**Ctrl+H**), or *Usage history…* in a card's menu, which opens it at that service.

At the top, pick a **Service** (the services that have history) and a **Period**: the last 7, 30
or 90 days. The window has these parts:

- **Over time**: one chart per meter. A limit is drawn as how much of it was used, from 0 to 100 %,
  red where it was used up. A balance is drawn as a line. A gap means nothing was recorded for more
  than 3 hours, usually because Lastdrop was not running.
- **Use per day**: how much was used each day, with the average and the highest day.
- **Active hours**: a grid of the 7 days of the week by the 24 hours of the day, darker where you
  used the most. It says which hour has the most use, such as *Most use: Tuesday 14:00–15:00*.
- **Tokens and cost**, for Claude Code and Codex only: tokens per day and per model, read from the
  session logs that Claude Code and Codex keep on your PC. The cost is what the same tokens would
  cost at the API's list prices. A subscription plan does not pay per token, so treat it as a
  measure of how much you used, not as a bill. Models without a known price are left out of the
  cost.

Prices come with Lastdrop, with the date they were checked. To change or add a price, put a
`prices.json` in `%APPDATA%\Lastdrop` in the same form as the
[bundled one](../../crates/core/prices.json): its models replace the bundled ones with the same
`id`, and new ones are added. It is read when the window opens.

### How history is kept

Lastdrop records the meters while it runs: at most one reading per meter every 5 minutes, plus one
when a limit resets or is used up. It keeps 90 days. History lives in
`%LOCALAPPDATA%\Lastdrop\history`, one small file per meter, and holds only the numbers. Nothing is
recorded while Lastdrop is not running, and history starts empty: the charts, the forecast and the
burn rate fill in as readings come in.

## Notifications

Lastdrop shows a Windows notification when:

- a limit reaches **80 %** or **95 %** used (you can pick other levels, or turn one off);
- a limit or a balance is **used up**;
- a limit **resets** after Lastdrop told you about it;
- a balance **runs low**: amber or red, as above.

Each level is announced once per window, and a restart does not repeat what was already
announced. When several things happen at once they share one notification (*+2 more*). Click a
notification to open the flyout. Windows' *Do not disturb* holds them back like any other
notification.

Change them in the settings under **Notifications**: a switch for all of them, the two levels
(*Off*, 50, 60, 70, 75, 80, 85, 90 or 95 %), and a check box for each of the other three. There is
no switch per service; to silence one, turn it off or hide its meters.

## How often Lastdrop reads

By default every 5 minutes. Under **Refresh usage** in the settings you can pick *Manual*, 1, 2,
5, 10, 15 or 30 minutes. In *Manual* Lastdrop reads once when it starts and then only when you
refresh.

Each service also has a minimum interval that wins over your choice, to stay gentle with the
service: 5 minutes for most, 10 for Antigravity. Codex's session files can be read every minute,
but its `app-server` is started at most every 15 minutes
([What Lastdrop reads](what-lastdrop-reads.md#codex)). Shortly after a limit's reset time, Lastdrop
reads once more to pick up the fresh numbers.
