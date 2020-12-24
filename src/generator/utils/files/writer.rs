use std::io::prelude::*;
use std::{fs::File, path::PathBuf};

use bytes::Bytes;

pub struct FileInfo {
	pub path: PathBuf,
	pub data: Bytes,
}

pub struct FileWriter {
	pub queue: Vec<FileInfo>,
	pub is_writing: bool,
}

impl FileWriter {
	pub fn new() -> FileWriter {
		FileWriter {
			queue: Vec::<FileInfo>::new(),
			is_writing: false,
		}
	}

	pub fn push_bytes(&mut self, path: PathBuf, data: Bytes) {
		println!("!!! Adding \"{}\" to queue, len is {}", path.clone().to_str().unwrap(), self.queue.len());
		// Remove from queue if an item with the same path already exists
		if let Some(item_index) = self.queue.iter().position(|f| f.path == path) {
			println!("!!! ... Found item, deleting");
			self.queue.remove(item_index);
			println!("!!! ... New queue len is {}", self.queue.len());
		}
		self.queue.push(FileInfo {
			path,
			data,
		});
		println!("!!! ... Final queue len is {}", self.queue.len());
		if !self.is_writing {
			self.write_next();
		}
	}

	fn write_next(&mut self) {
		if !self.is_writing && !self.queue.is_empty() {
			self.is_writing = true;
			let file_info = self.queue.remove(0);
			println!(
				"!!! Writing file \"{}\", new queue len is {}",
				file_info.path.clone().to_str().unwrap(),
				self.queue.len()
			);
			let mut output_file = File::create(file_info.path).expect("creating file");
			output_file.write(&file_info.data[..]).expect("writing file");
			self.is_writing = false;
			self.write_next();
		}
	}
}
