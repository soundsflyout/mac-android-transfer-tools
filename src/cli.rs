// Group together functions for cli outputs for easy testing.
pub mod pull;
pub mod push;
pub mod queue;

pub mod cli_tools {
    const MIB_OVER_KIB: i64 = 1_024;
    const GIB_OVER_KIB: i64 = 1_048_576;
    const TIB_OVER_KIB: i64 = 1_073_741_824;
    // Sets a buffer to leave 10 MiB of space left
    // on device after push/pull.
    const FILE_SPACE_BUFFER: i64 = 10_024;

    use adb_client::{ADBDeviceExt, server::ADBServer, server_device::ADBServerDevice};
    use dialoguer::{Confirm, MultiSelect};
    use std::error::Error;
    use std::path::Path;

    use crate::cli::pull::pull_tools;
    use crate::cli::push::push_tools;
    use crate::config::ConfigFile;
    use crate::*;

    enum FilesizeType {
        Kibibyte,
        Mebibyte,
        Gibibyte,
        Tebibyte,
    }

    impl FilesizeType {
        fn display(&self) -> String {
            match self {
                FilesizeType::Kibibyte => String::from("KiB"),
                FilesizeType::Mebibyte => String::from("MiB"),
                FilesizeType::Gibibyte => String::from("GiB"),
                FilesizeType::Tebibyte => String::from("TiB"),
            }
        }
    }

    fn filesize_type(input: i64) -> FilesizeType {
        let value = input.abs();
        match value {
            0..MIB_OVER_KIB => FilesizeType::Kibibyte,
            MIB_OVER_KIB..GIB_OVER_KIB => FilesizeType::Mebibyte,
            GIB_OVER_KIB..TIB_OVER_KIB => FilesizeType::Gibibyte,
            _ => FilesizeType::Tebibyte,
        }
    }

    fn human_readable(input: i64) -> f64 {
        let filesize = filesize_type(input);
        let finput = input as f64;
        match filesize {
            FilesizeType::Kibibyte => finput,
            FilesizeType::Mebibyte => finput / (MIB_OVER_KIB as f64),
            FilesizeType::Gibibyte => finput / (GIB_OVER_KIB as f64),
            FilesizeType::Tebibyte => finput / (TIB_OVER_KIB as f64),
        }
    }

    fn check_enough_space(addition: i64, free_space: i64) -> bool {
        if free_space < FILE_SPACE_BUFFER + addition {
            println!("Not enough space on disk");
            return false;
        }
        // This command could only error if interact got an
        // unexpected input, in which case the program *should*
        // crash.
        Confirm::new()
            .with_prompt("Do you want to make changes?")
            .interact()
            .expect("Unexpected input")
    }

    pub fn get_devices(server: &mut ADBServer) -> Result<Vec<String>, Box<dyn Error>> {
        let connected_devices: Vec<String> = server
            .devices()?
            .iter()
            .map(|x| x.identifier.clone())
            .collect();
        Ok(connected_devices)
    }

    pub fn get_storage_info(server: &mut ADBServer) -> Result<(), Box<dyn Error>> {
        let connected_devices: Vec<String> = server
            .devices()?
            .iter()
            .map(|x| x.identifier.clone())
            .collect();
        println!("{:?}", connected_devices);
        let selection = MultiSelect::new()
            .with_prompt("Choose device(s): \n Use up/down or k/j to move up/down and select with Space. Press Enter to confirm.")
            .items(&connected_devices)
            .interact()?;

        for idx in selection {
            let mut stdout = Vec::new();
            let mut device = server.get_device_by_name(&connected_devices[idx])?;
            device.shell_command(&"df -h", Some(&mut stdout), None)?;
            let stdout_str: String = String::from_utf8(stdout)?;
            println!("{}", stdout_str);
        }
        Ok(())
    }

    pub fn push(
        config: &ConfigFile,
        device: &mut ADBServerDevice,
        local_path: &Path,
        cli_input: &Cli,
    ) -> Result<(), Box<dyn Error>> {
        let Ok(queue) = push_tools::fetch_changes(
            config,
            device,
            local_path,
            cli_input.ignore_changes,
            cli_input.delete,
        ) else {
            println!("Can't grab files. Do both the local and remote directories exist?");
            return Ok(());
        };

        println!("Files to add: {}", queue.add);
        println!("Files to change: {}", queue.change);
        println!("Files to delete {}", queue.del);

        if queue.add == 0 && queue.change == 0 && (!cli_input.delete || queue.del == 0) {
            println!("No changes available. Exiting...");
            return Ok(());
        }

        let update_file_size_human_readable = human_readable(queue.total_size);
        let update_file_size_filesize_type = filesize_type(queue.total_size).display();
        println!(
            "Size of files to be changed: {:.2} {}",
            update_file_size_human_readable, update_file_size_filesize_type
        );

        let mut stdout = Vec::new();
        let shell_command: String = format!(r#"df "{}" | tail -n 1"#, config.remote_dir);
        device.shell_command(&shell_command, Some(&mut stdout), None)?;
        let stdout_str: String = String::from_utf8(stdout)?;
        let stdout_values: Vec<&str> = stdout_str.split_whitespace().collect();
        let free_space: i64 = stdout_values[3].parse()?;
        let free_human_readable = human_readable(free_space);
        let free_filesize_type = filesize_type(free_space).display();

        println!(
            "Space available: {:.2} {}",
            free_human_readable, free_filesize_type
        );

        let confirmation: bool = check_enough_space(queue.total_size, free_space);

        if confirmation {
            push_tools::write_changes(queue, config, device, local_path, cli_input.delete)?;
        } else {
            println!("Exiting...");
        }
        Ok(())
    }

    pub fn pull(
        config: &ConfigFile,
        device: &mut ADBServerDevice,
        local_path: &Path,
        cli_input: &Cli,
    ) -> Result<(), Box<dyn Error>> {
        let Ok(queue) = pull_tools::fetch_changes(
            config,
            device,
            local_path,
            cli_input.ignore_changes,
            cli_input.delete,
        ) else {
            println!("Can't grab files. Do both the local and remote directories exist?");
            return Ok(());
        };

        println!("Files to add: {}", queue.add);
        println!("Files to change: {}", queue.change);
        println!("Files to delete {}", queue.del);

        if queue.add == 0 && queue.change == 0 && (!cli_input.delete || queue.del == 0) {
            println!("No changes available. Exiting...");
            return Ok(());
        }

        let update_file_size_human_readable = human_readable(queue.total_size);
        let update_file_size_filesize_type = filesize_type(queue.total_size).display();
        println!(
            "Size of files to be changed: {:.2} {}",
            update_file_size_human_readable, update_file_size_filesize_type
        );
        let unix_command: String =
            format!(r#"df -k "{}" | tail -n 1"#, local_path.to_str().unwrap());

        let output = Command::new("sh")
            .arg("-c")
            .arg(&unix_command)
            .stdout(Stdio::piped())
            .output()?;

        let stdout_str = String::from_utf8(output.stdout)?;
        let stdout_values: Vec<&str> = stdout_str.split_whitespace().collect();

        let free_space: i64 = stdout_values[3].parse()?;

        let free_human_readable = human_readable(free_space);
        let free_filesize_type = filesize_type(free_space).display();

        println!(
            "Space available: {:.2} {}",
            free_human_readable, free_filesize_type
        );

        let confirmation: bool = check_enough_space(queue.total_size, free_space);
        if confirmation {
            pull_tools::write_changes(queue, config, device, local_path, cli_input.delete)?;
        } else {
            println!("Exiting...");
        }
        Ok(())
    }
}
