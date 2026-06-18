CREATE INDEX IF NOT EXISTS idx_endpoints_device_expiry
ON endpoints (device_id, expires_at);
