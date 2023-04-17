use std::path::PathBuf;

use std::io;

use std::fs;
use std::io::BufReader;
use std::io::BufRead;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

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

impl Hash for SrcFile {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // TODO: should contain date, checksum, etc.
        self.path.hash(state);
    }
}

impl PartialEq for SrcFile {
    fn eq(&self, other: &Self) -> bool {
        let mut a = DefaultHasher::new();
        let mut b = DefaultHasher::new();
        self.hash(&mut a);
        other.hash(&mut b);

        a.finish() == b.finish()
    }
}

impl SrcFile {
    pub fn simple_hash(&self) -> u64 {
        let mut a = DefaultHasher::new();
        self.hash(&mut a);

        a.finish()
    }

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

pub struct BinaryFile {
    pub path: PathBuf,
    //hash: Hash,

    pub contents: Option<Vec<u8>>,
    pub decompiled_src: Option<Vec<String>>
}
