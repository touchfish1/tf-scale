use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use reqwest::{Method, Response, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tfscale_core::protocol::{CreateAuthKeyResponse, DeviceSummary, RenameDeviceRequest};

#[derive(Debug, Parser)]
#[command(name = "tfscalectl", version, about = "tf-scale operator CLI")]
struct Cli {
    #[arg(
        long,
        env = "TFSCALE_CONTROL_URL",
        default_value = "http://127.0.0.1:8080"
    )]
    control_url: String,

    #[arg(long, env = "TFSCALE_ADMIN_TOKEN")]
    admin_token: Option<String>,

    #[arg(long)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    AuthKey {
        #[command(subcommand)]
        command: AuthKeyCommand,
    },
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AuthKeyCommand {
    Create,
}

#[derive(Debug, Subcommand)]
enum DeviceCommand {
    List,
    Rename { device_id: String, hostname: String },
    Delete { device_id: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let client = reqwest::Client::new();

    match cli.command {
        Command::AuthKey {
            command: AuthKeyCommand::Create,
        } => {
            let response: CreateAuthKeyResponse = request_json(
                &client,
                Method::POST,
                &cli.control_url,
                "/v1/auth-keys",
                None,
            )
            .await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&response)?);
            } else {
                println!("{}", response.key);
            }
        }
        Command::Device {
            command: DeviceCommand::List,
        } => {
            let devices: Vec<DeviceSummary> =
                request_json(&client, Method::GET, &cli.control_url, "/v1/devices", None).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&devices)?);
            } else {
                print_devices(&devices);
            }
        }
        Command::Device {
            command:
                DeviceCommand::Rename {
                    device_id,
                    hostname,
                },
        } => {
            let path = format!("/v1/devices/{device_id}");
            let payload = serde_json::to_value(RenameDeviceRequest { hostname })?;
            let device: DeviceSummary = request_json(
                &client,
                Method::PATCH,
                &cli.control_url,
                &path,
                Some(payload),
            )
            .await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&device)?);
            } else {
                println!("renamed {} to {}", device.id, device.hostname);
            }
        }
        Command::Device {
            command: DeviceCommand::Delete { device_id },
        } => {
            let path = format!("/v1/devices/{device_id}");
            request_empty(&client, Method::DELETE, &cli.control_url, &path).await?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "deleted": true,
                        "device_id": device_id
                    })
                );
            } else {
                println!("deleted {device_id}");
            }
        }
    }

    Ok(())
}

async fn request_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    method: Method,
    control_url: &str,
    path: &str,
    payload: Option<serde_json::Value>,
) -> Result<T> {
    let response = send_request(client, method, control_url, path, payload).await?;
    Ok(response
        .json()
        .await
        .context("failed to decode response JSON")?)
}

async fn request_empty(
    client: &reqwest::Client,
    method: Method,
    control_url: &str,
    path: &str,
) -> Result<()> {
    send_request(client, method, control_url, path, None).await?;
    Ok(())
}

async fn send_request(
    client: &reqwest::Client,
    method: Method,
    control_url: &str,
    path: &str,
    payload: Option<serde_json::Value>,
) -> Result<Response> {
    let url = format!("{}{}", control_url.trim_end_matches('/'), path);
    let mut request = client.request(method, url);
    if let Some(payload) = payload {
        request = request.json(&payload);
    }

    let response = request.send().await.context("request failed")?;
    ensure_success(response).await
}

async fn ensure_success(response: Response) -> Result<Response> {
    if response.status().is_success() {
        return Ok(response);
    }

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if let Ok(error) = serde_json::from_str::<ApiErrorResponse>(&body) {
        bail!(
            "control plane returned {}: {}",
            status_text(status),
            error.error
        );
    }

    if body.trim().is_empty() {
        bail!("control plane returned {}", status_text(status));
    }

    bail!(
        "control plane returned {}: {}",
        status_text(status),
        body.trim()
    );
}

fn status_text(status: StatusCode) -> String {
    format!(
        "{} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("HTTP error")
    )
}

#[derive(Deserialize)]
struct ApiErrorResponse {
    error: String,
}

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

fn print_devices(devices: &[DeviceSummary]) {
    println!(
        "{:<28} {:<20} {:<15} {:<10} {:<8} {:<10} LAST_SEEN",
        "ID", "HOSTNAME", "IPV4", "OS", "ARCH", "BACKEND"
    );

    for device in devices {
        println!(
            "{:<28} {:<20} {:<15} {:<10} {:<8} {:<10} {}",
            device.id,
            device.hostname,
            device.ipv4,
            device.os,
            device.arch,
            device.backend_type,
            device.last_seen_at.as_deref().unwrap_or("-")
        );
    }
}
