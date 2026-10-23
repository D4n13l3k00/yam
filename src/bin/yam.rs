use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use reqwest::blocking::{multipart, Client};
use serde_json::Value;
use std::{fs, path::PathBuf};

#[derive(Parser)]
#[command(name = "yam", about = "YAM package manager")]
struct Cli {
    #[arg(long, env = "YAM_URL", default_value = "http://127.0.0.1:8099")]
    url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Install {
        package: PathBuf,
    },
    List,
    Ps,
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Run {
        id: String,
        #[arg(short, long)]
        detach: bool,
    },
    Stop {
        id: String,
    },
    Rm {
        id: String,
    },
    Logs {
        id: String,
    },
    Inspect {
        id: String,
    },
    Device,
    Led {
        #[command(subcommand)]
        command: LedCommand,
    },
}

#[derive(Subcommand)]
enum LedCommand {
    Set { colors: Vec<String> },
    Off,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::new();
    match cli.command {
        Command::Install { package } => {
            let file =
                fs::File::open(&package).with_context(|| format!("open {}", package.display()))?;
            let part = multipart::Part::reader(file).file_name(
                package
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
            );
            let response = client
                .post(endpoint(&cli.url, "/api/plugins"))
                .multipart(multipart::Form::new().part("package", part))
                .send()?;
            print_response(response)?;
        }
        Command::List => {
            let packages: Value =
                json_response(client.get(endpoint(&cli.url, "/api/plugins")).send()?)?;
            println!("{}", serde_json::to_string_pretty(&packages)?);
        }
        Command::Ps => {
            let packages: Value =
                json_response(client.get(endpoint(&cli.url, "/api/plugins")).send()?)?;
            let empty = Vec::new();
            let running = packages
                .as_array()
                .unwrap_or(&empty)
                .iter()
                .filter(|package| package["running"] == true)
                .collect::<Vec<_>>();
            println!("{}", serde_json::to_string_pretty(&running)?);
        }
        Command::Enable { id } => set_enabled(&client, &cli.url, &id, true)?,
        Command::Disable { id } => set_enabled(&client, &cli.url, &id, false)?,
        Command::Run { id, detach } => {
            print_response(
                client
                    .post(endpoint(&cli.url, &format!("/api/plugins/{id}/invoke")))
                    .send()?,
            )?;
            if !detach {
                wait_for_package(&client, &cli.url, &id)?;
            }
        }
        Command::Stop { id } => print_response(
            client
                .post(endpoint(&cli.url, &format!("/api/plugins/{id}/stop")))
                .send()?,
        )?,
        Command::Rm { id } => print_response(
            client
                .delete(endpoint(&cli.url, &format!("/api/plugins/{id}")))
                .send()?,
        )?,
        Command::Logs { id } => print_response(
            client
                .get(endpoint(&cli.url, &format!("/api/plugins/{id}/logs")))
                .send()?,
        )?,
        Command::Inspect { id } => {
            let packages: Value =
                json_response(client.get(endpoint(&cli.url, "/api/plugins")).send()?)?;
            let package = packages
                .as_array()
                .and_then(|items| items.iter().find(|item| item["manifest"]["id"] == id))
                .context("package not found")?;
            println!("{}", serde_json::to_string_pretty(package)?);
        }
        Command::Device => print_response(client.get(endpoint(&cli.url, "/api/device")).send()?)?,
        Command::Led { command } => match command {
            LedCommand::Set { colors } => print_response(
                client
                    .post(endpoint(&cli.url, "/api/led"))
                    .json(&serde_json::json!({"colors":colors}))
                    .send()?,
            )?,
            LedCommand::Off => {
                print_response(client.post(endpoint(&cli.url, "/api/led/off")).send()?)?
            }
        },
    }
    Ok(())
}

fn endpoint(base: &str, path: &str) -> String {
    format!("{}{}", base.trim_end_matches('/'), path)
}

fn json_response(response: reqwest::blocking::Response) -> Result<Value> {
    let response = response.error_for_status()?;
    Ok(response.json()?)
}

fn print_response(response: reqwest::blocking::Response) -> Result<()> {
    let status = response.status();
    let text = response.text()?;
    if !status.is_success() {
        bail!("{}: {}", status, text)
    }
    if !text.trim().is_empty() {
        println!("{text}");
    }
    Ok(())
}

fn set_enabled(client: &Client, url: &str, id: &str, desired: bool) -> Result<()> {
    let packages = json_response(client.get(endpoint(url, "/api/plugins")).send()?)?;
    let enabled = packages
        .as_array()
        .and_then(|items| items.iter().find(|item| item["manifest"]["id"] == id))
        .context("package not found")?["enabled"]
        .as_bool()
        .unwrap_or(false);
    if enabled != desired {
        print_response(
            client
                .post(endpoint(url, &format!("/api/plugins/{id}/toggle")))
                .send()?,
        )?;
    }
    Ok(())
}

fn wait_for_package(client: &Client, url: &str, id: &str) -> Result<()> {
    loop {
        let packages = json_response(client.get(endpoint(url, "/api/plugins")).send()?)?;
        let running = packages
            .as_array()
            .and_then(|items| items.iter().find(|item| item["manifest"]["id"] == id))
            .context("package disappeared")?["running"]
            .as_bool()
            .unwrap_or(false);
        if !running {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
