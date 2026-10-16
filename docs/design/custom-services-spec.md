# Lastdrop — services described in JSON

Implementation spec (M2, ROADMAP "Simple balance APIs": "Declarative provider files (JSON:
endpoint, auth, field mapping) — no code needed"). Companions: [readout-spec.md](readout-spec.md)
(`R§`: meters, balances, R§6.4 monograms), [flyout-spec.md](flyout-spec.md) (`F§`). Code:
`lastdrop_core::custom` (§2–§4), `lastdrop_providers::custom` (§5), the app's loading (§6).

A service with a simple usage or balance endpoint and an API key is described in a JSON file:
where to ask, how to send the key, and where the numbers are in the answer. Lastdrop ships a few
(§7); a user adds more by dropping a file in `%APPDATA%\Lastdrop\providers\`. No code is compiled
or run: the file is data (ARCHITECTURE "Slint licence rule": provider definitions are data).

## 1. What a described service is

A service like any other: its own entry in Settings ▸ Services (turned off until the user turns it
on), its own card, readout rows, history and notifications, a monogram (R§6.4), and an API key in
Settings ▸ API keys, kept in Credential Manager as `Lastdrop:<id>:<account>:api_key`. It has one
account. It is read at most every 5 minutes.

## 2. The file

```json
{
  "format": 1,
  "id": "elevenlabs",
  "name": "ElevenLabs",
  "monogram": "E",
  "url": "https://api.elevenlabs.io/v1/user/subscription",
  "key_header": "xi-api-key",
  "key_prefix": "",
  "usage_page": "https://elevenlabs.io/app/subscription",
  "meters": [
    { "id": "characters", "label": "Chars", "short_name": "characters", "name": "Characters",
      "used": "/character_count", "limit": "/character_limit",
      "resets_at": "/next_character_count_reset_unix", "window": "month" }
  ],
  "balances": [
    { "id": "credits", "label": "Cr", "short_name": "credits", "name": "Credits",
      "amount": "/data/total_credits", "minus": "/data/total_usage", "currency": "USD" }
  ]
}
```

| Field | Required | Meaning |
|---|---|---|
| `format` | yes | `1`. A file of another format is skipped. |
| `id` | yes | 1–32 of `a-z 0-9 _ -`, starting with a letter; not a built-in service's id; unique. Settings, history file names and credential names use it, so it must never change. |
| `name` | yes | The service's name as shown, 1–32 characters. |
| `monogram` | no | 1–2 characters (R§6.4); default the name's first letter. |
| `url` | yes | `https://` only, at most 512 characters. Asked with `GET`. |
| `key_header` | no | The header that carries the key; default `Authorization`. Not `Host`, `Cookie`, `Content-Length` or a header that starts with `Proxy-`. |
| `key_prefix` | no | Put before the key; default `Bearer ` with `Authorization`, else empty. |
| `usage_page` | no | `https://` page opened from the card (F§9). |
| `meters` | — | Window meters (below). |
| `balances` | — | Balances (below). At least one meter or balance. |

Every meter and balance has `id` (as the service's `id`), `label` (≤ 5 characters, the readout's
column), `short_name` and `name` (R§2's three names, English, as the file gives them).

A **meter** says how much of a limit is used:

| Field | Meaning |
|---|---|
| `used` + `limit` | JSON pointers (RFC 6901) to numbers; used ÷ limit. |
| `used_percent` | instead: a pointer to a percent, 0–100. |
| `resets_at` | a pointer to when it resets: an RFC 3339 instant or epoch seconds or milliseconds. |
| `window` | `month` (a calendar month), `week`, `day`, or `<n>h` / `<n>d` (1–9999). |

A **balance** says how much is left:

| Field | Meaning |
|---|---|
| `amount` | a pointer to a number. |
| `minus` | optional: a pointer to a number taken off it (`total_credits − total_usage`). |
| `scale` | optional: multiplies it (`0.01` for cents). |
| `currency` | an ISO 4217 code (`USD`); or `currency_at`, a pointer to one; without either the amount is in credits. |

Numbers may be JSON numbers or numbers in strings (`"110.00"`). A pointer that finds nothing, or
not a number, gives no value for that meter or balance: it is left out, never 0 (ARCHITECTURE
"Honest data"). An answer that gives none of the file's meters and balances is a parse error.

## 3. Loading

At start-up both processes read the bundled files (§7), then every `*.json` in
`%APPDATA%\Lastdrop\providers\` (at most 64 files of at most 64 KiB, sorted by name). A user file
with a bundled file's `id` replaces it. A file that does not follow §2 is skipped and the log says
which file and why (never its contents). The described services are known for the life of the
process: a file added or changed takes effect at the next start.

## 4. Identity in the code

A described service is `ProviderId::Custom(CustomId)`, an index into the services loaded at
start-up (`lastdrop_core::custom`). It is written as its `id` wherever a service id is written
(settings, the state file, history file names, credential names), so it survives restarts; a
service whose file is gone is kept in settings as an entry this build does not know
(ARCHITECTURE "Settings": unknown entries are kept) and shows nowhere.

## 5. Reading

`GET url` with the key header and `Accept: application/json`, a 10 s timeout. 200 → the meters and
balances of §2; 401 → sign-in (the API-key message, F§4.3: no vendor client); 403 and 429 → back
off, `Retry-After` honoured; 5xx → transient; anything else → parse error. No key stored → the
API-key message. The answer is read only for the pointers of the file and dropped.

## 6. Settings

Each described service is in Settings ▸ Services and has a row in Settings ▸ API keys. The hint of
the services section says that more can be described in `%APPDATA%\Lastdrop\providers\`.

Saving or removing a key tells the tray process which service it was by 24 bits of the hash of
the file id, not by its place in the list: the dialog process may have read a file the tray
process has not (added since it started). The tray process lifts that service's sign-in stop;
a service it does not know waits for the next start.

## 7. Bundled

| File | Service | Reads |
|---|---|---|
| `deepseek.json` | DeepSeek | `GET https://api.deepseek.com/user/balance`: `balance_infos[0].total_balance` in `balance_infos[0].currency` |
| `elevenlabs.json` | ElevenLabs | `GET https://api.elevenlabs.io/v1/user/subscription` (`xi-api-key`): characters used of the limit, reset at `next_character_count_reset_unix` |
| `openrouter.json` | OpenRouter | `GET https://openrouter.ai/api/v1/credits` with a management key: `total_credits − total_usage` in USD |
