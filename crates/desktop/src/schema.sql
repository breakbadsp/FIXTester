CREATE TABLE IF NOT EXISTS profile (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    sender TEXT NOT NULL,
    target TEXT NOT NULL,
    role INTEGER NOT NULL,
    address TEXT NOT NULL,
    heartbeat INTEGER NOT NULL,
    next_send INTEGER NOT NULL DEFAULT 1 CHECK(next_send>0),
    next_receive INTEGER NOT NULL DEFAULT 1 CHECK(next_receive>0),
    reset INTEGER NOT NULL DEFAULT 0,
    UNIQUE(version,sender,target)
);
CREATE TABLE IF NOT EXISTS history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    profile INTEGER NOT NULL REFERENCES profile(id),
    time_ms INTEGER NOT NULL,
    direction INTEGER NOT NULL,
    kind TEXT NOT NULL,
    seq TEXT NOT NULL,
    summary TEXT NOT NULL,
    status TEXT NOT NULL,
    raw BLOB NOT NULL,
    transferred INTEGER NOT NULL,
    application INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS history_profile ON history(profile,id);
CREATE INDEX IF NOT EXISTS history_age ON history(time_ms);
CREATE TABLE IF NOT EXISTS order_values (
    profile INTEGER NOT NULL REFERENCES profile(id),
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    quantity TEXT NOT NULL,
    price TEXT NOT NULL,
    recent INTEGER NOT NULL,
    PRIMARY KEY(profile,symbol)
);
CREATE TABLE IF NOT EXISTS serial (
    id INTEGER PRIMARY KEY CHECK(id=1),
    value INTEGER NOT NULL
);
INSERT OR IGNORE INTO serial VALUES(1,0);
