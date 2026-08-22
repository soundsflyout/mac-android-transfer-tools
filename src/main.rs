pub mod cli;
pub mod config;

use adb_client::server::ADBServer;
use clap::Parser;
use serde_json::value::Value;
use std::env;
use std::error::Error;
use std::fs::File;
use std::process::Command;
use std::process::Stdio;

use crate::cli::cli_tools::{get_devices, get_storage_info, pull, push};
use crate::config::ConfigFile;

#[derive(Parser)]
pub struct Cli {
    command: String, //push or pull
    alias: Option<String>,

    /// only update new files.
    #[arg(short, long, default_value_t = false)]
    ignore_changes: bool,

    /// delete files in target that are not in source.
    #[arg(short, long, default_value_t = false)]
    delete: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli_input = Cli::parse();

    if !(cfg!(target_os = "macos") || cfg!(target_os = "linux")) {
        println!("Unsupported OS. Only MacOS and Linux are currently supported. Exiting...");
        return Ok(());
    }

    let mut commands = vec!["devices", "storage", "push", "pull"].into_iter();
    if !commands.any(|x| cli_input.command == x) {
        println!("Invalid input. Exiting...");
        return Ok(());
    }

    let mut server = ADBServer::default();

    if cli_input.command == "devices" {
        println!("Connected devices:");
        for device_id in get_devices(&mut server)? {
            println!("{}", device_id);
        }
        return Ok(());
    }

    if cli_input.command == "storage" {
        get_storage_info(&mut server)?;
        return Ok(());
    }

    let mut config_path = match env::home_dir() {
        Some(path) => path,
        None => panic!("No root path found. Exiting..."),
    };
    config_path.push(".config/adb-sync-tool/config.json");

    let config_file = match File::open(config_path) {
        Ok(file) => file,
        Err(_) => {
            println!(
                "Cannot find config file. Please read example_config.json or the readme for details. Exiting..."
            );
            return Ok(());
        }
    };

    let Some(cli_alias) = &cli_input.alias else {
        panic!("Error: Missing alias")
    };

    let mut alias: Value = serde_json::from_reader(config_file).expect("Cannot read config file");
    let config: ConfigFile = serde_json::from_value(alias[cli_alias].take())?;

    let mut device = match &config.device_name {
        None => match server.get_device() {
            Ok(device) => device,
            Err(_) => {
                println!(
                    "Cannot find device. Please make sure that only one device is connected. Exiting..."
                );
                return Ok(());
            }
        },
        Some(name) => match server.get_device_by_name(name) {
            Ok(device) => device,
            Err(_) => {
                println!(
                    "Cannot find device. Please make sure that device is connected. Exiting..."
                );
                return Ok(());
            }
        },
    };

    let mut local_path = match env::home_dir() {
        Some(path) => path,
        None => panic!("No root path found"),
    };
    local_path.push(&config.local_dir);

    if cli_input.command == "push" {
        push(&config, &mut device, &local_path, &cli_input)?;
    }

    if cli_input.command == "pull" {
        pull(&config, &mut device, &local_path, &cli_input)?;
    }
    Ok(())
}
