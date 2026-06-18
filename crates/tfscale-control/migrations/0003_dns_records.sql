CREATE TABLE IF NOT EXISTS dns_records (
    id TEXT PRIMARY KEY,
    network_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    value TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_dns_records_network_name_type
ON dns_records(network_id, lower(name), type);

CREATE UNIQUE INDEX IF NOT EXISTS idx_dns_records_device_type
ON dns_records(device_id, type);
