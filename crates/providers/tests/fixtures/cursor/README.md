# Fixtures: cursor

Invented copies of the Cursor app's `state.vscdb` (an SQLite database), for the read-only
reader in `src/sqlite.rs` and `cursor::read_token`:

| File | Shows |
|---|---|
| `state-signed-in.vscdb` | `ItemTable(key, value)` with 300 filler rows (so the table has interior pages), a 5,000-byte value (overflow pages), an invented session under `cursorAuth/accessToken` and an invented `cursorAuth/cachedEmail`, plus a `cursorDiskKV` table as noise |
| `state-signed-out.vscdb` | the same without the `cursorAuth/*` rows |
| `state-wal.vscdb` + `state-wal.vscdb-wal` | a database in WAL mode whose session is only in the write-ahead log (made with `wal_autocheckpoint = 0`, a `TRUNCATE` checkpoint of the filler rows, then the session committed and both files copied while the connection was open) |
| `state-cycle.vscdb` | `state-signed-in.vscdb` with `ItemTable`'s interior root page listing itself as its first child (a damaged file the walk must stop on) |

**Every value here is invented.** The files were made with Python's `sqlite3` module (page size
1024; the first two in rollback mode and `VACUUM`ed), never copied from a machine: the session is an unsigned JWT
`{"sub": "github|user_test0000000000000000", "exp": 4102444800, ...}` with the signature
`test-signature-not-real`, the address is under `example.invalid`. The script, for regenerating
them:

```python
import base64, json, sqlite3

def b64(data): return base64.urlsafe_b64encode(data).rstrip(b"=").decode()
def jwt(claims): return ".".join([b64(b'{"alg":"HS256","typ":"JWT"}'),
                                  b64(json.dumps(claims).encode()), "test-signature-not-real"])

def make(path, token):
    db = sqlite3.connect(path)
    db.execute("PRAGMA page_size = 1024")
    db.execute("PRAGMA journal_mode = DELETE")
    db.execute("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)")
    db.execute("CREATE TABLE cursorDiskKV (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)")
    for i in range(300):
        db.execute("INSERT INTO ItemTable VALUES (?, ?)", (f"invented.setting/{i:03}", "x" * (i % 40)))
    db.execute("INSERT INTO ItemTable VALUES (?, ?)", ("invented.big", "B" * 5000))
    if token is not None:
        db.execute("INSERT INTO ItemTable VALUES (?, ?)", ("cursorAuth/accessToken", token))
        db.execute("INSERT INTO ItemTable VALUES (?, ?)", ("cursorAuth/cachedEmail", "tester@example.invalid"))
    for i in range(50):
        db.execute("INSERT INTO cursorDiskKV VALUES (?, ?)", (f"invented.kv/{i}", b"\x00" * 100))
    db.commit(); db.execute("VACUUM"); db.close()

token = jwt({"sub": "github|user_test0000000000000000", "exp": 4102444800,
             "iss": "https://authentication.cursor.sh", "type": "session"})
make("state-signed-in.vscdb", token)
make("state-signed-out.vscdb", None)
```

The usage-summary bodies and the other services' answers are written inline in
`tests/new_services.rs`, invented the same way.
