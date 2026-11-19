-- Charge raw bytes, variable text, and a fixed allowance for row metadata.
ALTER TABLE history ADD COLUMN charge INTEGER GENERATED ALWAYS AS (
    128 + length(raw) + length(CAST(kind AS BLOB)) + length(CAST(seq AS BLOB))
        + length(CAST(summary AS BLOB)) + max(16, length(CAST(status AS BLOB)))
) VIRTUAL;

CREATE TABLE history_quota (
    id INTEGER PRIMARY KEY CHECK(id=1),
    bytes INTEGER NOT NULL CHECK(bytes>=0)
);
INSERT INTO history_quota SELECT 1,coalesce(sum(charge),0) FROM history;

CREATE TRIGGER history_charge_insert AFTER INSERT ON history BEGIN
    UPDATE history_quota SET bytes=bytes+new.charge WHERE id=1;
END;
CREATE TRIGGER history_charge_delete AFTER DELETE ON history BEGIN
    UPDATE history_quota SET bytes=bytes-old.charge WHERE id=1;
END;
CREATE TRIGGER history_charge_update AFTER UPDATE ON history
WHEN new.charge<>old.charge BEGIN
    UPDATE history_quota SET bytes=bytes+new.charge-old.charge WHERE id=1;
END;

PRAGMA user_version=1;
