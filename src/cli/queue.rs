use std::collections::HashSet;
use std::path::PathBuf;

pub struct Queue {
    pub dir_queue: Vec<PathBuf>,
    pub file_queue: Vec<PathBuf>,
    pub del_queue: HashSet<String>,
    pub add: u64,
    pub change: u64,
    pub del: u64,
    pub total_size: i64,
}
