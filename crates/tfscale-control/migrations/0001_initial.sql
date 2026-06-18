CREATE TABLE IF NOT EXISTS auth_keys (
    id TEXT PRIMARY KEY,
    key_hash TEXT NOT NULL UNIQUE,
    description TEXT,
    expires_at TEXT,
    used_at TEXT,
    reusable INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    revoked_at TEXT
);

CREATE TABLE IF NOT EXISTS devices (
    id TEXT PRIMARY KEY,
    network_id TEXT NOT NULL,
    hostname TEXT NOT NULL,
    machine_key TEXT NOT NULL UNIQUE,
    node_key TEXT NOT NULL,
    backend_type TEXT NOT NULL,
    backend_public_credential TEXT NOT NULL,
    ipv4 TEXT NOT NULL UNIQUE,
    os TEXT NOT NULL,
    arch TEXT NOT NULL,
    client_version TEXT NOT NULL,
    last_seen_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted_at TEXT
);

CREATE TABLE IF NOT EXISTS ip_allocations (
    id TEXT PRIMARY KEY,
    network_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    ip TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    released_at TEXT
);

CREATE TABLE IF NOT EXISTS network_backends (
    id TEXT PRIMARY KEY,
    network_id TEXT NOT NULL,
    backend_type TEXT NOT NULL,
    capabilities TEXT NOT NULL DEFAULT '{}',
    config TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS endpoints (
    id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    address TEXT NOT NULL,
    port INTEGER NOT NULL,
    protocol TEXT NOT NULL,
    source TEXT,
    latency_ms INTEGER,
    last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
