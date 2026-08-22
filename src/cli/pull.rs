pub mod pull_tools {
    use adb_client::{ADBDeviceExt, server_device::ADBServerDevice};
    use console::style;
    use indicatif::ProgressBar;
    use std::collections::HashSet;
    use std::error::Error;
    use std::fs::metadata;
    use std::fs::write;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::str::Lines;
    use std::time::UNIX_EPOCH;

    use crate::cli::queue::Queue;
    use crate::config::ConfigFile;

    fn modified_time(local_path: &Path) -> Result<u64, Box<dyn Error>> {
        match metadata(local_path) {
            Ok(path_metadata) => Ok(path_metadata
                .modified()?
                .duration_since(UNIX_EPOCH)?
                .as_secs()),
            _ => Ok(0),
        }
    }

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

        let mut stdout = Vec::new();
        let shell_command: String = format!("find {} -type f", config.remote_dir);
        device.shell_command(&shell_command, Some(&mut stdout), None)?;
        let stdout_str: String = String::from_utf8(stdout)?;
        let files: Lines = stdout_str.lines();

        let scan_length: u64 = stdout_str.bytes().filter(|&b| b == b'\n').count() as u64;

        let abs_local_path = local_path.to_str().unwrap();
        println!("Fetching changes...");
        if delete {
            let shell_command: String = format!("find {} -type f", abs_local_path);
            let output = Command::new("sh").arg("-c").arg(&shell_command).output()?;
            let stdout_str: String = String::from_utf8(output.stdout)?;
            let files: Lines = stdout_str.lines();
            for file in files {
                queue.del_queue.insert(String::from(file));
            }
        }

        let loading_bar = ProgressBar::new(scan_length);

        for entry in files {
            let remote_path = PathBuf::from(entry);
            let remote_path_str = remote_path.to_str().unwrap();

            let rel_path = remote_path.strip_prefix(&config.remote_dir)?;

            //shadow the local_path input since it is not needed outside of here.
            let mut local_path = local_path.to_path_buf();
            local_path.push(rel_path);
            let modified_time: u64 = modified_time(&local_path)?;
            let local_file_size: i64 = match metadata(&local_path) {
                Ok(metadata) => metadata.len() as i64,
                Err(_) => 0,
            };

            if delete {
                queue.del_queue.remove(local_path.to_str().unwrap());
            }

            let remote_mod_time = device.stat(remote_path_str)?.mod_time as u64;

            //checks if the file or any of its parent directories are hidden.
            let is_hidden: bool = remote_path.to_str().unwrap().contains("/.");

            if (config.allow_hidden || !is_hidden)
                && (modified_time == 0 || (!ignore_changes && modified_time < remote_mod_time))
            {
                let parent_dir = local_path.parent().expect("Cannot find parent directory");
                let path_buf = PathBuf::from(parent_dir);
                let is_added: bool = match queue.dir_queue.last() {
                    Some(x) => x == parent_dir,
                    None => false,
                };
                if !is_added {
                    queue.dir_queue.push(path_buf);
                }
                if modified_time == 0 {
                    queue.add += 1;
                    queue.total_size += device.stat(remote_path_str)?.file_size as i64;
                } else {
                    queue.change += 1;
                    queue.total_size +=
                        device.stat(remote_path_str)?.file_size as i64 - local_file_size;
                }
                queue.file_queue.push(remote_path);
            }

            loading_bar.inc(1);
        }
        if delete {
            for file in &queue.del_queue {
                queue.del += 1;
                queue.total_size -= metadata(file)?.len() as i64
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

        if delete && queue.del > 0 {
            println!("Deleting excess files...");
            let delete_loader = ProgressBar::new(queue.del);
            for file in queue.del_queue {
                let mut curr_path: PathBuf = PathBuf::from(file);
                let cmd = format!("rm -r {}", curr_path.to_str().unwrap());
                Command::new("sh").arg("-c").arg(cmd).output()?;
                curr_path = PathBuf::from(curr_path.parent().expect("Missing parent"));

                // We need to delete any empty parent directories.
                while curr_path.read_dir()?.count() == 0 {
                    let cmd = format!("rm -r {}", curr_path.to_str().unwrap());
                    Command::new("sh").arg("-c").arg(cmd).output()?;
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
            let local_path_str = path.to_str().unwrap();
            let cmd = format!(r#"mkdir -p "{}""#, local_path_str);
            Command::new("sh").arg("-c").arg(cmd).output()?;
            directory_loader.inc(1);
        }
        directory_loader.finish();

        for path in queue.file_queue {
            let mut curr_local_path = PathBuf::from(&local_path);

            let rel_path = path.strip_prefix(&config.remote_dir)?;

            let rel_path_str = rel_path.to_str().unwrap();

            curr_local_path.push(rel_path);
            let local_path_str = curr_local_path.to_str().unwrap();

            let modified_time = modified_time(&curr_local_path)? as u32;
            let remote_path_str = String::from(path.to_str().unwrap());
            let remote_mod_time = device.stat(&remote_path_str)?.mod_time;

            // Since this is unix time, a value of 0 means the file does not exist.
            if modified_time < remote_mod_time {
                let add_or_update: &str = match modified_time {
                    0 => "Adding",
                    _ => "Updating",
                };
                let curr_idx_str = format!("[{}/{}]", curr_idx, total);
                let push_message = format!("{} {}", add_or_update, rel_path_str);
                println!("{} {}", style(curr_idx_str).bold().dim(), push_message);
                let mut stdout = Vec::new();
                device.pull(&remote_path_str, &mut stdout)?;
                write(local_path_str, stdout)?;
                curr_idx += 1;
            }
        }
        Ok(())
    }
}
