# Adding your own service

Some services report a balance or a usage limit over a simple web API that takes an API key.
Lastdrop can read such a service without any new code: you describe it in a small JSON file, and
it gets a card in the flyout, rows in the taskbar readout, history and notifications like any
other service.

Lastdrop comes with three of these files: DeepSeek, ElevenLabs and OpenRouter. They show in
Settings ▸ Services next to the built-in services.

The file is data only. Lastdrop never runs code from it: it asks one web address, with your key in
one header, and picks numbers out of the answer.

## What you need

- An API endpoint that answers a plain `GET` request with JSON.
- An `https://` address for it.
- An API key that the service accepts in a request header.
- The answer must hold the numbers you want as numbers, or as numbers in text (`"110.00"`).

If the service needs a sign-in flow, a cookie, a `POST` request or several requests, a JSON file
cannot describe it.

## Where the file goes

Put the file in this folder. Create the folder if it is not there:

```
%APPDATA%\Lastdrop\providers
```

Type that path into the File Explorer address bar to open it, or choose **Open folder** under
**More services** in Settings ▸ Services, which also creates it. Any file name that ends in
`.json` works, for example `example-ai.json`.

Lastdrop reads the folder **when it starts**. After you add or change a file, quit Lastdrop from its
tray menu (*Quit Lastdrop*) and start it again.

Limits: at most 64 files are read, in name order, and a file larger than 64 KiB is skipped.

## A worked example

Say a service called Example AI has a usage endpoint at `https://api.example.com/v1/usage`. You
send your key as `Authorization: Bearer <key>`, and it answers like this (made-up numbers):

```json
{
  "plan": {
    "requests_used": 1240,
    "requests_limit": 5000,
    "period_end": "2026-11-01T00:00:00Z"
  },
  "credit": {
    "remaining_cents": "1250"
  }
}
```

The answer has two things worth showing: a monthly request limit (1,240 of 5,000 used, resets on
1 November) and a prepaid credit (1,250 cents, so 12.50 USD). This file describes both:

```json
{
  "format": 1,
  "id": "example-ai",
  "name": "Example AI",
  "monogram": "EX",
  "url": "https://api.example.com/v1/usage",
  "usage_page": "https://example.com/account/usage",
  "meters": [
    {
      "id": "requests",
      "label": "Req",
      "short_name": "requests",
      "name": "Monthly requests",
      "used": "/plan/requests_used",
      "limit": "/plan/requests_limit",
      "resets_at": "/plan/period_end",
      "window": "month"
    }
  ],
  "balances": [
    {
      "id": "credit",
      "label": "Cr",
      "short_name": "credit",
      "name": "Prepaid credit",
      "amount": "/credit/remaining_cents",
      "scale": 0.01,
      "currency": "USD"
    }
  ]
}
```

Save it as `%APPDATA%\Lastdrop\providers\example-ai.json`, then:

1. Restart Lastdrop.
2. Open Settings ▸ Services. *Example AI* is in the list.
3. Open its row with the arrow at the end, paste your Example AI key and choose **Save key**.
   Saving the key also turns the service on.
4. Open the flyout. Example AI has a card with a *Monthly requests* bar and a *Prepaid credit*
   balance.

No header is named in the file, so Lastdrop sends the key as `Authorization: Bearer <key>`.

## Finding the numbers: JSON pointers

Each number is found with a JSON pointer: the path from the top of the answer, with a `/` before
each step.

| In the answer | Pointer |
|---|---|
| `plan` ▸ `requests_used` | `/plan/requests_used` |
| the first item of the list `balance_infos`, then `total_balance` | `/balance_infos/0/total_balance` |
| a key that itself holds a `/`, such as `a/b` | `/a~1b` |

List items count from 0. If a pointer finds nothing, or finds something that is not a number,
Lastdrop leaves that meter or balance out of the reading. It never shows a missing number as 0.

## The fields

### The service

| Field | Needed | What it does |
|---|---|---|
| `format` | yes | Always `1`. |
| `id` | yes | A short name for the service: 1–32 characters of `a`–`z`, `0`–`9`, `_` and `-`, starting with a letter. It cannot be the id of a built-in service (`claude`, `codex` and so on). Settings, history and the stored key are filed under it, so do not change it later. |
| `name` | yes | The name shown in Lastdrop, 1–32 characters. |
| `monogram` | no | 1–2 characters drawn as the service's icon. Without it, the first letter of the name. |
| `url` | yes | The address Lastdrop asks with `GET`. Must start with `https://`, at most 512 characters, no `user@` part. |
| `key_header` | no | The request header that carries the key. Without it, `Authorization`. `Host`, `Cookie`, `Content-Length`, `Transfer-Encoding`, `Connection` and headers starting with `Proxy-` are not allowed. |
| `key_prefix` | no | Text put before the key, at most 32 characters. Without it, `Bearer ` (with the space) when the header is `Authorization`, else nothing. |
| `usage_page` | no | An `https://` page that *Open usage page* in the card's right-click menu opens. |
| `meters` | see below | A list of usage limits. |
| `balances` | see below | A list of balances. |

A file needs at least one meter or balance, and at most 16 in all.

Every meter and every balance has these four:

| Field | What it does |
|---|---|
| `id` | Its id, with the same rules as the service's `id`. Each id appears once in the file. |
| `label` | At most 5 characters: the column heading in the taskbar readout. |
| `short_name` | At most 40 characters, used inside sentences: "Example AI *requests*". |
| `name` | At most 40 characters, the full name shown on the card. |

The names are shown as written; Lastdrop does not translate them.

### A meter: how much of a limit is used

Give either `used` and `limit`, or `used_percent`:

| Field | What it does |
|---|---|
| `used` and `limit` | Pointers to two numbers. The bar shows `used ÷ limit`. |
| `used_percent` | Instead: a pointer to a percent used, 0–100. |
| `resets_at` | Optional: a pointer to when the limit resets, as a date and time with a time zone (`2026-11-01T00:00:00Z`, `2026-11-01T01:00:00+01:00`) or as seconds or milliseconds since 1970. |
| `window` | Required: how long one period of the limit is. `month` (a calendar month), `week`, `day`, or a number of hours or days such as `5h` or `30d` (1–9999). |

With a reset time and a window length, Lastdrop can work out how much of the period has gone, and
so tell you whether you are ahead of pace and when the limit would run out
([Reading the flyout](reading-the-flyout.md)).

### A balance: how much is left

| Field | What it does |
|---|---|
| `amount` | A pointer to the amount. |
| `minus` | Optional: a pointer to a number taken off the amount, for an answer that gives a total and a used part (`total_credits` minus `total_usage`). |
| `scale` | Optional: a positive number the amount is multiplied by, such as `0.01` for an answer in cents. |
| `currency` | Optional: a three-letter currency code in capitals, such as `USD` or `EUR`. |
| `currency_at` | Instead of `currency`: a pointer to the code in the answer. |

With neither `currency` nor `currency_at`, the amount is shown as credits.

A balance's forecast ("lasts about 3 weeks") comes from how fast it went down over the last 7
days, so it needs two days of history first.

## The bundled files as examples

The three files Lastdrop ships are short and show the common shapes. Copy one into your providers
folder to start from it. A file in your folder with the same `id` as a bundled one replaces it.

- [`deepseek.json`](../../crates/app/providers/deepseek.json): a balance whose currency is in the
  answer (`currency_at`), sent with the default `Authorization: Bearer` header.
- [`elevenlabs.json`](../../crates/app/providers/elevenlabs.json): a monthly limit with a reset
  time, the key in the `xi-api-key` header without a prefix.
- [`openrouter.json`](../../crates/app/providers/openrouter.json): a balance worked out as a total
  minus what was used. OpenRouter's credits endpoint needs a management key, not an ordinary API
  key.

## The API key

You enter the key in the service's row of Settings ▸ Services: open the row with the arrow at
its end. Its line says *Needs an API key* until one is stored. Lastdrop stores the key in Windows
Credential Manager, never in a file, and the settings window never shows it again. In Credential
Manager it is a generic credential named `Lastdrop:<id>:default:api_key`, for example
`Lastdrop:example-ai:default:api_key`.

To change the key, paste a new one and choose **Save key**. **Remove** deletes it from Credential
Manager.

## How Lastdrop reads it

- It sends `GET` to the file's `url` with the key header and `Accept: application/json`, and gives
  up after 10 seconds.
- It reads at your refresh interval, but never more often than every 5 minutes.
- It takes the numbers at the file's pointers and drops the rest of the answer. History keeps only
  those numbers.
- It sends nothing else: no data about you, your PC or your other services.

What the answer's status code does:

| Answer | What happens |
|---|---|
| 2xx | The numbers are read. If none of the file's pointers finds a number, it counts as an error. |
| 401 | The key is wrong or has expired. The card says *API key missing or rejected* with an *Enter key…* link, and Lastdrop stops asking until you save a key again or refresh by hand. |
| 403 or 429 | Lastdrop backs off: at least 5 minutes, doubling after repeated failures up to an hour, and longer if the service asks for it with `Retry-After` (up to 24 hours). The card keeps the last numbers and says when it tries next. |
| 5xx | Treated as a passing problem, with the same back-off. |
| anything else | An error on the card. |

No stored key gives the same *API key missing or rejected* message as a 401.

## When a file does not load

A file that does not follow these rules is skipped as a whole, and the service does not appear in
Settings. Lastdrop writes the file name and the reason to its log, never the file's contents. Open
`%LOCALAPPDATA%\Lastdrop\logs\lastdrop.log` and look for lines that start with `providers:`, such
as:

```
providers: example-ai.json left out: meter requests: window: month, week, day, <n>h or <n>d
```

Common causes:

- A field that is not in the tables above. Unknown fields are not ignored: they make the file fail,
  and so do comments, which JSON does not allow.
- A meter without `window`.
- A `label` longer than 5 characters.
- A pointer without the leading `/`.
- An `http://` address.
- An `id` with capital letters or spaces, or one that another file in your folder already uses.

The log line names the first problem only. Fix it, restart Lastdrop and check the log again.

If you delete a file, the service disappears at the next start. Its settings stay in
`settings.json`, so it comes back as it was if you put the file back.
