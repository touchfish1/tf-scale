use anyhow::Result;
use clap::{Parser, Subcommand};
use tfscale_core::protocol::{CreateAuthKeyResponse, DeviceSummary};

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
            let response: CreateAuthKeyResponse = client
                .post(format!("{}/v1/auth-keys", cli.control_url))
                .send()
                .await?
                .error_for_status()?
                .json()
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
            let devices: Vec<DeviceSummary> = client
                .get(format!("{}/v1/devices", cli.control_url))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&devices)?);
            } else {
                print_devices(&devices);
            }
        }
        Command::Device {
            command: DeviceCommand::Delete { device_id },
        } => {
            client
                .delete(format!("{}/v1/devices/{device_id}", cli.control_url))
                .send()
                .await?
                .error_for_status()?;
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
