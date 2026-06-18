use anyhow::Result as AnyResult;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool, sqlite::SqlitePoolOptions};
use std::net::SocketAddr;
use tfscale_core::{
    DeviceId, NetworkId,
    protocol::{
        CreateAuthKeyResponse, DeviceSummary, EndpointPayload, EndpointProbeRequest,
        EndpointProbeResponse, HeartbeatRequest, HeartbeatResponse, NetworkMapPeer,
        NetworkMapResponse, NetworkMapSelf, RegisterDeviceRequest, RegisterDeviceResponse,
        RenameDeviceRequest,
    },
};
use tracing::{info, warn};
use uuid::Uuid;

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "endpoint_metadata",
        sql: include_str!("../migrations/0002_endpoint_metadata.sql"),
    },
];

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

#[derive(Debug, Parser)]
#[command(name = "tfscaled", version, about = "tf-scale control plane")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value = "./tf-scale.db")]
        db: String,

        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,
    },
}

#[tokio::main]
async fn main() -> AnyResult<()> {
    init_tracing();
    let cli = Cli::parse();

    match cli.command {
        Command::Serve { db, listen } => {
            serve(db, listen).await?;
        }
    }

    Ok(())
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
}

async fn serve(db: String, listen: String) -> AnyResult<()> {
    let database_url = format!("sqlite://{db}?mode=rwc");
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;
    init_schema(&pool).await?;

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/auth-keys", post(create_auth_key))
        .route("/v1/devices", get(list_devices))
        .route(
            "/v1/devices/{device_id}",
            delete(delete_device).patch(rename_device),
        )
        .route("/v1/agent/register", post(register_device))
        .route("/v1/agent/heartbeat", post(heartbeat))
        .route("/v1/agent/network-map", get(network_map))
        .route("/v1/agent/endpoint-probe", post(endpoint_probe))
        .with_state(AppState { pool });

    let addr: SocketAddr = listen.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(%addr, "control plane listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn init_schema(pool: &SqlitePool) -> AnyResult<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(pool)
    .await?;

    for migration in MIGRATIONS {
        if migration_applied(pool, migration.version).await? {
            continue;
        }

        let mut tx = pool.begin().await?;
        for statement in migration_statements(migration.sql) {
            sqlx::query(statement).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO schema_migrations (version, name) VALUES (?, ?)")
            .bind(migration.version)
            .bind(migration.name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }

    Ok(())
}

async fn migration_applied(pool: &SqlitePool, version: i64) -> AnyResult<bool> {
    let applied: Option<i64> =
        sqlx::query_scalar("SELECT version FROM schema_migrations WHERE version = ?")
            .bind(version)
            .fetch_optional(pool)
            .await?;

    Ok(applied.is_some())
}

fn migration_statements(sql: &str) -> impl Iterator<Item = &str> {
    sql.split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

async fn create_auth_key(
    State(state): State<AppState>,
) -> std::result::Result<Json<CreateAuthKeyResponse>, ApiError> {
    let id = format!("ak_{}", Uuid::now_v7().simple());
    let key = format!("tfk_{}", Uuid::now_v7().simple());
    let key_hash = hash_secret(&key);

    sqlx::query("INSERT INTO auth_keys (id, key_hash) VALUES (?, ?)")
        .bind(&id)
        .bind(key_hash)
        .execute(&state.pool)
        .await?;

    Ok(Json(CreateAuthKeyResponse { id, key }))
}

async fn list_devices(
    State(state): State<AppState>,
) -> std::result::Result<Json<Vec<DeviceSummary>>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT id, hostname, ipv4, os, arch, backend_type, last_seen_at
        FROM devices
        WHERE deleted_at IS NULL
        ORDER BY hostname ASC
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    let devices = rows
        .into_iter()
        .map(|row| DeviceSummary {
            id: row.get("id"),
            hostname: row.get("hostname"),
            ipv4: row.get("ipv4"),
            os: row.get("os"),
            arch: row.get("arch"),
            backend_type: row.get("backend_type"),
            last_seen_at: row.get("last_seen_at"),
        })
        .collect();

    Ok(Json(devices))
}

async fn delete_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> std::result::Result<StatusCode, ApiError> {
    let result = sqlx::query(
        r#"
        UPDATE devices
        SET deleted_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(device_id)
    .execute(&state.pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("device not found"));
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn rename_device(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Json(request): Json<RenameDeviceRequest>,
) -> std::result::Result<Json<DeviceSummary>, ApiError> {
    let hostname = normalize_hostname(&request.hostname)?;
    ensure_hostname_available(&state.pool, &device_id, &hostname).await?;

    let result = sqlx::query(
        r#"
        UPDATE devices
        SET hostname = ?, updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(&hostname)
    .bind(&device_id)
    .execute(&state.pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("device not found"));
    }

    Ok(Json(load_device_summary(&state.pool, &device_id).await?))
}

async fn register_device(
    State(state): State<AppState>,
    Json(request): Json<RegisterDeviceRequest>,
) -> std::result::Result<Json<RegisterDeviceResponse>, ApiError> {
    if let Some(existing) = find_existing_device(&state.pool, &request.machine_key).await? {
        return Ok(Json(existing));
    }

    validate_auth_key(&state.pool, &request.auth_key).await?;

    let network_id = NetworkId::from("net_default".to_string());
    let device_id = DeviceId::new();
    let node_key = format!("node_{}", Uuid::now_v7().simple());
    let ipv4 = allocate_ipv4(&state.pool).await?;

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO devices (
            id, network_id, hostname, machine_key, node_key, backend_type,
            backend_public_credential, ipv4, os, arch, client_version, last_seen_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
        "#,
    )
    .bind(device_id.as_str())
    .bind(network_id.as_str())
    .bind(&request.hostname)
    .bind(&request.machine_key)
    .bind(&node_key)
    .bind(&request.backend_type)
    .bind(&request.backend_public_credential)
    .bind(&ipv4)
    .bind(&request.os)
    .bind(&request.arch)
    .bind(&request.client_version)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO ip_allocations (id, network_id, device_id, ip, state)
        VALUES (?, ?, ?, ?, 'assigned')
        "#,
    )
    .bind(format!("ip_{}", Uuid::now_v7().simple()))
    .bind(network_id.as_str())
    .bind(device_id.as_str())
    .bind(&ipv4)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE auth_keys
        SET used_at = COALESCE(used_at, CURRENT_TIMESTAMP)
        WHERE key_hash = ?
        "#,
    )
    .bind(hash_secret(&request.auth_key))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(RegisterDeviceResponse {
        device_id: device_id.to_string(),
        node_key,
        ipv4,
        network_id: network_id.to_string(),
        poll_interval_seconds: 5,
    }))
}

async fn load_device_summary(
    pool: &SqlitePool,
    device_id: &str,
) -> std::result::Result<DeviceSummary, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, hostname, ipv4, os, arch, backend_type, last_seen_at
        FROM devices
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Err(ApiError::not_found("device not found"));
    };

    Ok(DeviceSummary {
        id: row.get("id"),
        hostname: row.get("hostname"),
        ipv4: row.get("ipv4"),
        os: row.get("os"),
        arch: row.get("arch"),
        backend_type: row.get("backend_type"),
        last_seen_at: row.get("last_seen_at"),
    })
}

async fn ensure_hostname_available(
    pool: &SqlitePool,
    device_id: &str,
    hostname: &str,
) -> std::result::Result<(), ApiError> {
    let existing: Option<String> = sqlx::query_scalar(
        r#"
        SELECT id
        FROM devices
        WHERE lower(hostname) = lower(?) AND id != ? AND deleted_at IS NULL
        "#,
    )
    .bind(hostname)
    .bind(device_id)
    .fetch_optional(pool)
    .await?;

    if existing.is_some() {
        return Err(ApiError::conflict("hostname already in use"));
    }

    Ok(())
}

fn normalize_hostname(value: &str) -> std::result::Result<String, ApiError> {
    let hostname = value.trim().to_ascii_lowercase();

    if hostname.is_empty() {
        return Err(ApiError::bad_request("hostname is required"));
    }

    if hostname.len() > 63 {
        return Err(ApiError::bad_request(
            "hostname must be 63 characters or fewer",
        ));
    }

    if hostname.starts_with('-') || hostname.ends_with('-') {
        return Err(ApiError::bad_request(
            "hostname cannot start or end with '-'",
        ));
    }

    if !hostname
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(ApiError::bad_request(
            "hostname can contain only lowercase letters, digits, and '-'",
        ));
    }

    Ok(hostname)
}

async fn heartbeat(
    State(state): State<AppState>,
    Json(request): Json<HeartbeatRequest>,
) -> std::result::Result<Json<HeartbeatResponse>, ApiError> {
    validate_device_node_key(&state.pool, &request.device_id, &request.node_key).await?;

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE devices
        SET last_seen_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(&request.device_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM endpoints WHERE device_id = ?")
        .bind(&request.device_id)
        .execute(&mut *tx)
        .await?;

    for endpoint in request.endpoints {
        sqlx::query(
            r#"
            INSERT INTO endpoints (
                id, device_id, kind, address, port, protocol, source, priority, expires_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(format!("ep_{}", Uuid::now_v7().simple()))
        .bind(&request.device_id)
        .bind(&endpoint.kind)
        .bind(&endpoint.address)
        .bind(i64::from(endpoint.port))
        .bind(&endpoint.protocol)
        .bind(endpoint.source.as_deref().unwrap_or("local"))
        .bind(endpoint.priority.map(i64::from))
        .bind(endpoint.expires_at)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(Json(HeartbeatResponse {
        ok: true,
        network_map_version: network_map_version(&state.pool).await?,
    }))
}

async fn endpoint_probe(
    State(state): State<AppState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    Json(request): Json<EndpointProbeRequest>,
) -> std::result::Result<Json<EndpointProbeResponse>, ApiError> {
    validate_device_node_key(&state.pool, &request.device_id, &request.node_key).await?;

    if request.protocol != "udp" {
        return Err(ApiError::bad_request(
            "only udp endpoint probing is supported",
        ));
    }

    Ok(Json(EndpointProbeResponse {
        observed_address: addr.ip().to_string(),
        observed_port: addr.port(),
        protocol: request.protocol,
    }))
}

async fn network_map(
    State(state): State<AppState>,
    Query(query): Query<NetworkMapQuery>,
) -> std::result::Result<Json<NetworkMapResponse>, ApiError> {
    validate_device_node_key(&state.pool, &query.device_id, &query.node_key).await?;

    let self_row = sqlx::query(
        r#"
        SELECT id, hostname, ipv4, backend_type
        FROM devices
        WHERE id = ? AND deleted_at IS NULL
        "#,
    )
    .bind(&query.device_id)
    .fetch_one(&state.pool)
    .await?;

    let peer_rows = sqlx::query(
        r#"
        SELECT id, hostname, ipv4, backend_type, backend_public_credential
        FROM devices
        WHERE id != ? AND deleted_at IS NULL
        ORDER BY hostname ASC
        "#,
    )
    .bind(&query.device_id)
    .fetch_all(&state.pool)
    .await?;

    let mut peers = Vec::with_capacity(peer_rows.len());
    for row in peer_rows {
        let peer_id: String = row.get("id");
        let ipv4: String = row.get("ipv4");
        peers.push(NetworkMapPeer {
            device_id: peer_id.clone(),
            hostname: row.get("hostname"),
            ipv4: ipv4.clone(),
            backend_type: row.get("backend_type"),
            backend_public_credential: row.get("backend_public_credential"),
            endpoints: load_endpoints(&state.pool, &peer_id).await?,
            allowed_routes: vec![format!("{ipv4}/32")],
        });
    }

    Ok(Json(NetworkMapResponse {
        network_map_version: network_map_version(&state.pool).await?,
        self_device: NetworkMapSelf {
            device_id: self_row.get("id"),
            hostname: self_row.get("hostname"),
            ipv4: self_row.get("ipv4"),
            backend_type: self_row.get("backend_type"),
        },
        peers,
    }))
}

async fn validate_auth_key(pool: &SqlitePool, auth_key: &str) -> std::result::Result<(), ApiError> {
    let key_hash = hash_secret(auth_key);
    let row = sqlx::query(
        r#"
        SELECT reusable, used_at, revoked_at
        FROM auth_keys
        WHERE key_hash = ?
        "#,
    )
    .bind(key_hash)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        warn!("device registration rejected because auth key was not found");
        return Err(ApiError::unauthorized("invalid auth key"));
    };

    let reusable: i64 = row.get("reusable");
    let used_at: Option<String> = row.get("used_at");
    let revoked_at: Option<String> = row.get("revoked_at");

    if revoked_at.is_some() {
        return Err(ApiError::unauthorized("auth key revoked"));
    }

    if reusable == 0 && used_at.is_some() {
        return Err(ApiError::unauthorized("auth key already used"));
    }

    Ok(())
}

async fn find_existing_device(
    pool: &SqlitePool,
    machine_key: &str,
) -> std::result::Result<Option<RegisterDeviceResponse>, ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id, node_key, ipv4, network_id
        FROM devices
        WHERE machine_key = ? AND deleted_at IS NULL
        "#,
    )
    .bind(machine_key)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| RegisterDeviceResponse {
        device_id: row.get("id"),
        node_key: row.get("node_key"),
        ipv4: row.get("ipv4"),
        network_id: row.get("network_id"),
        poll_interval_seconds: 5,
    }))
}

async fn validate_device_node_key(
    pool: &SqlitePool,
    device_id: &str,
    node_key: &str,
) -> std::result::Result<(), ApiError> {
    let row = sqlx::query(
        r#"
        SELECT id
        FROM devices
        WHERE id = ? AND node_key = ? AND deleted_at IS NULL
        "#,
    )
    .bind(device_id)
    .bind(node_key)
    .fetch_optional(pool)
    .await?;

    if row.is_none() {
        return Err(ApiError::unauthorized("invalid device credentials"));
    }

    Ok(())
}

async fn load_endpoints(
    pool: &SqlitePool,
    device_id: &str,
) -> std::result::Result<Vec<EndpointPayload>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT kind, address, port, protocol, source, priority, expires_at
        FROM endpoints
        WHERE device_id = ?
          AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)
        ORDER BY kind ASC, address ASC
        "#,
    )
    .bind(device_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| EndpointPayload {
            kind: row.get("kind"),
            address: row.get("address"),
            port: row.get::<i64, _>("port") as u16,
            protocol: row.get("protocol"),
            source: row.get("source"),
            priority: row
                .get::<Option<i64>, _>("priority")
                .map(|value| value as i32),
            expires_at: row.get("expires_at"),
        })
        .collect())
}

async fn network_map_version(pool: &SqlitePool) -> std::result::Result<i64, ApiError> {
    let device_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE deleted_at IS NULL")
            .fetch_one(pool)
            .await?;
    let endpoint_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM endpoints")
        .fetch_one(pool)
        .await?;
    let endpoint_timestamp_sum: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(unixepoch(last_seen_at)), 0) FROM endpoints")
            .fetch_one(pool)
            .await?;

    Ok(device_count + endpoint_count + endpoint_timestamp_sum)
}

async fn allocate_ipv4(pool: &SqlitePool) -> std::result::Result<String, ApiError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ip_allocations")
        .fetch_one(pool)
        .await?;
    let host_octet = count + 2;

    if host_octet > 254 {
        return Err(ApiError::internal("default MVP address pool is exhausted"));
    }

    Ok(format!("100.64.0.{host_octet}"))
}

fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
}

#[derive(Deserialize)]
struct NetworkMapQuery {
    device_id: String,
    node_key: String,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(value: sqlx::Error) -> Self {
        Self::internal(value.to_string())
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_hostname() {
        let hostname = normalize_hostname(" Dev-Box ").expect("hostname");

        assert_eq!(hostname, "dev-box");
    }

    #[test]
    fn rejects_empty_hostname() {
        let error = normalize_hostname("   ").expect_err("empty hostname should fail");

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_hostname_with_invalid_characters() {
        let error = normalize_hostname("dev_box").expect_err("invalid hostname should fail");

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_hostname_with_edge_hyphen() {
        let error = normalize_hostname("-devbox").expect_err("invalid hostname should fail");

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn splits_migration_statements() {
        let statements =
            migration_statements("CREATE TABLE one (id TEXT); ; CREATE TABLE two (id TEXT);")
                .collect::<Vec<_>>();

        assert_eq!(
            statements,
            vec!["CREATE TABLE one (id TEXT)", "CREATE TABLE two (id TEXT)"]
        );
    }

    #[test]
    fn registers_endpoint_metadata_migration() {
        assert_eq!(MIGRATIONS.last().expect("migration").version, 2);
        assert_eq!(
            MIGRATIONS.last().expect("migration").name,
            "endpoint_metadata"
        );
    }
}
