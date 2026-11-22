CREATE TABLE pair_tickets (
    ticket TEXT PRIMARY KEY,
    expires_at INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending'
);

CREATE TABLE devices (
    id TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    credential TEXT NOT NULL UNIQUE,
    revoked INTEGER NOT NULL DEFAULT 0
);
