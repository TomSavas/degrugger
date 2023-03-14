use std::path::PathBuf;

use std::io;

use std::fs;
use std::io::BufReader;
use std::io::BufRead;

use std::collections::HashMap;

//pub trait File {
//    fn load_contents(&mut self) -> io::Result<bool>;
//    //fn new(path: PathBuf, load_contents: bool) -> io::Result<impl File>;
//    fn new(path: PathBuf, load_contents: bool) -> io::Result<impl File>;
//}

pub struct SrcFile {
    pub path: PathBuf,
    //hash: Hash,

    pub lines: Option<Vec<String>>,
    pub line_to_addr: HashMap<usize, u64>,
    pub addr_to_line: HashMap<u64, usize>,
}

//impl File for SrcFile {
impl SrcFile {
    pub fn load_contents(&mut self) -> io::Result<bool> {
        let file = fs::File::open(&self.path)?;
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().filter_map(io::Result::ok).collect();
        self.lines = Some(lines);

        // Whether the file contents were updated or not
        Ok(true)
    }

    pub fn new(path: PathBuf, load_contents: bool) -> io::Result<SrcFile> {
        let mut src_file = SrcFile{ path: path, lines: None, line_to_addr: HashMap::new(), addr_to_line: HashMap::new() };
        if load_contents {
            src_file.load_contents()?;
        }

        Ok(src_file)
    }
}

//pub struct BinaryFile {
//    pub path: PathBuf,
//    //hash: Hash,
//
//    pub contents: Option<Vec<u8>>,
//}
