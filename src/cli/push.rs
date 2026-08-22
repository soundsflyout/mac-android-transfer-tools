pub mod push_tools {
    use adb_client::{ADBDeviceExt, server_device::ADBServerDevice};
    use console::style;
    use indicatif::ProgressBar;
    use std::collections::HashSet;
    use std::error::Error;
    use std::fs::{File, metadata};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::str::Lines;
    use std::time::UNIX_EPOCH;

    use crate::cli::queue::Queue;
    use crate::config::ConfigFile;

    pub fn fetch_changes(
        config: &ConfigFile,
        device: &mut ADBServerDevice,
        local_path: &Path,
        ignore_changes: bool,
        delete: bool,
    ) -> Result<Queue, Box<dyn Error>> {
        let mut queue = Queue {
            dir_queue: Vec::new(),
            file_queue: Vec::new(),
            del_queue: HashSet::new(),
            add: 0,
            change: 0,
            del: 0,
            total_size: 0,
        };

        let abs_local_path = local_path.to_str().unwrap();

        let shell_command: String = format!("find {} -type f", abs_local_path);
        let output = Command::new("sh").arg("-c").arg(&shell_command).output()?;
        let stdout_str = String::from_utf8(output.stdout)?;
        let files: Lines = stdout_str.lines();

        let mut del_queue = HashSet::new();

        println!("Fetching changes...");
        if delete {
            let mut stdout = Vec::new();
            let shell_command: String = format!("find {} -type f", config.remote_dir);
            device.shell_command(&shell_command, Some(&mut stdout), None)?;
            let stdout_str: String = String::from_utf8(stdout)?;
            let files: Lines = stdout_str.lines();
            for file in files {
                del_queue.insert(String::from(file));
            }
        }

        let scan_length: u64 = match stdout_str
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
            .checked_sub(1)
        {
            Some(x) => x as u64,
            None => 0,
        };

        let loading_bar = ProgressBar::new(scan_length);

        for entry in files {
            let local_path = PathBuf::from(entry);

            let rel_path = local_path.strip_prefix(abs_local_path)?;

            let mut remote_path = PathBuf::from(&config.remote_dir);
            remote_path.push(rel_path);
            let remote_path_str = remote_path.to_str().unwrap();
            let remote_file_size = device.stat(remote_path_str)?.file_size as i64;

            if delete {
                del_queue.remove(remote_path_str);
            }

            let modified_time = device.stat(remote_path_str)?.mod_time as u64;

            let path_metadata = metadata(&local_path)?;
            let local_mod_time = path_metadata
                .modified()?
                .duration_since(UNIX_EPOCH)?
                .as_secs();

            //checks if the file or any of its parent directories are hidden.
            let is_hidden = local_path.to_str().unwrap().contains("/.");

            if (config.allow_hidden || !is_hidden)
                && (modified_time == 0 || (!ignore_changes && modified_time < local_mod_time))
            {
                let parent_dir = remote_path.parent().expect("Cannot find parent directory");
                let path_buf = PathBuf::from(parent_dir);
                let is_added: bool = match queue.dir_queue.last() {
                    Some(x) => x == parent_dir,
                    None => false,
                };
                if !is_added
                    & (config.allow_hidden
                        || !path_buf
                            .file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with('.'))
                {
                    queue.dir_queue.push(path_buf);
                }
                if modified_time == 0 {
                    queue.add += 1;
                    queue.total_size += path_metadata.len() as i64;
                } else {
                    queue.change += 1;
                    queue.total_size += path_metadata.len() as i64 - remote_file_size;
                }
                queue.file_queue.push(local_path);
            }
            loading_bar.inc(1);
        }

        if delete {
            for file in &del_queue {
                queue.del += 1;
                queue.total_size -= device.stat(file)?.file_size as i64
            }
        }
        loading_bar.finish();

        // Convert file in bytes to corresponding kiB
        queue.total_size /= 1024;

        Ok(queue)
    }

    pub fn write_changes(
        queue: Queue,
        config: &ConfigFile,
        device: &mut ADBServerDevice,
        local_path: &Path,
        delete: bool,
    ) -> Result<(), Box<dyn Error>> {
        let total: u64 = queue.add + queue.change;
        let mut curr_idx = 1;

        let abs_local_path = local_path.to_str().unwrap();

        if delete && queue.del > 0 {
            println!("Deleting excess files...");
            let delete_loader = ProgressBar::new(queue.del);
            // We need to delete the file, and any empty parent directories.
            for file in queue.del_queue {
                let mut curr_path: PathBuf = PathBuf::from(file);
                while device.list(curr_path.to_str().unwrap())?.is_empty() {
                    let cmd = format!("rm -r {}", curr_path.to_str().unwrap());
                    device.shell_command(&cmd, None, None)?;
                    curr_path = match curr_path.parent() {
                        Some(path) => PathBuf::from(path),
                        None => break,
                    }
                }
                delete_loader.inc(1);
            }
            delete_loader.finish();
        }

        let directory_loader = ProgressBar::new(queue.dir_queue.len() as u64);
        println!("Initializing directories...");
        for path in queue.dir_queue {
            let remote_path_str = path.to_str().unwrap();
            let cmd = format!(r#"mkdir -p "{}""#, remote_path_str);
            device.shell_command(&cmd, None, None)?;
            directory_loader.inc(1);
        }
        directory_loader.finish();

        for path in queue.file_queue {
            let path_metadata = metadata(&path)?;
            let mut remote_path = PathBuf::from(&config.remote_dir);

            let rel_path = path.strip_prefix(abs_local_path)?;

            let rel_path_str = rel_path.to_str().unwrap();

            remote_path.push(rel_path);
            let remote_path_str = remote_path.to_str().unwrap();

            let modified_time = device.stat(remote_path_str)?.mod_time as u64;
            // Since this is unix time, a value of 0 means the file does not exist.
            if modified_time
                < path_metadata
                    .modified()?
                    .duration_since(UNIX_EPOCH)?
                    .as_secs()
            {
                let file_path = File::open(&path)?;
                let add_or_update: &str = match modified_time {
                    0 => "Adding",
                    _ => "Updating",
                };
                let curr_idx_str = format!("[{}/{}]", curr_idx, total);
                let push_message = format!("{} {}", add_or_update, rel_path_str);
                println!("{} {}", style(curr_idx_str).bold().dim(), push_message);
                device.push(file_path, remote_path_str)?;
                curr_idx += 1;
            }
        }
        Ok(())
    }
}
