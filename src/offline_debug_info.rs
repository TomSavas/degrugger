use std::collections::HashMap;
use std::marker::Send;
use std::io::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{ AtomicBool, Ordering };
use std::sync::mpsc::{ channel, Receiver, Sender };
use std::thread::{ Builder, JoinHandle };

use crate::SrcFile;

pub struct OfflineAddr(usize);

struct DecompiledSrcFile {
    decompiled_src: Vec<String>,
    src_to_decompiled_mapping: HashMap<usize, Vec<usize>>,
}

struct BreakableSrcLocation {
    addr: OfflineAddr,
    src_line: usize,
    src_col: usize,
    decompiled_line: usize,
}

pub struct SrcFileDebugInfo {
    decompiled_src: DecompiledSrcFile,
    breakable_locations: Vec<BreakableSrcLocation>,
}

trait Worker {
    fn work(&mut self);
}

struct DwarfInfo<'a> {
    bin_data: Arc<Vec<u8>>,
    dwarf_cow: Arc<Option<gimli::Dwarf<std::borrow::Cow<'a, [u8]>>>>,
    dwarf: Arc<Option<gimli::Dwarf<EndianSlice<'a, RunTimeEndian>>>>,
}

struct OfflineDebugInfoWorker<'a> {
    request_receiver: Receiver<Arc<SrcFile>>,
    response_sender: Sender<Arc<SrcFileDebugInfo>>,

    exec_path: PathBuf,
    //dwarf: Option<DwarfInfo>,
    dwarf: DwarfInfo<'a>,
}

// TEMP
use gimli::*;
use object::Object;
use object::ObjectSection;

impl<'a> OfflineDebugInfoWorker<'a> {
    pub fn new(exec_path: PathBuf) -> (Self, Sender<Arc<SrcFile>>, Receiver<Arc<SrcFileDebugInfo>>) {
        let (request_sender, request_receiver) = channel();
        let (response_sender, response_receiver) = channel();

        (Self{ request_receiver: request_receiver, response_sender: response_sender, exec_path: exec_path, dwarf: DwarfInfo{ bin_data: Arc::new(vec![]), dwarf_cow: Arc::new(None), dwarf: Arc::new(None) } }, request_sender, response_receiver)
    }

    fn gather_dwarf_info(&mut self) {
        println!("Analysing dwarf...");

        let endian = gimli::RunTimeEndian::Little;
        let bin_data = Arc::new(std::fs::read(self.exec_path.clone()).unwrap());;

        let object_owned = object::File::parse(&**self.dwarf.bin_data).unwrap();
        let object = &object_owned;

        let load_section = |id: gimli::SectionId| -> std::result::Result<std::borrow::Cow<[u8]>, gimli::Error> {
            match object.section_by_name(id.name()) {
                Some(ref section) => Ok(section
                                        .uncompressed_data()
                                        .unwrap_or(std::borrow::Cow::Borrowed(&[][..]))),
                None => Ok(std::borrow::Cow::Borrowed(&[][..])),
            }
        };

        // Load all of the sections.
        //let dwarf_cow = gimli::Dwarf::load(&load_section).unwrap();
        self.dwarf.dwarf_cow = Arc::new(Some(gimli::Dwarf::load(&load_section).unwrap()));

    }

    fn decompile_src(src_file: &SrcFile) -> DecompiledSrcFile {
        DecompiledSrcFile{ decompiled_src: vec![], src_to_decompiled_mapping: HashMap::new() }
    }

    fn generate_breakable_src_locations(&mut self, src_file: &SrcFile) -> Vec<BreakableSrcLocation> {
        //let dwarf = self.dwarf.dwarf.as_ref().as_ref().unwrap();
        let endian = gimli::RunTimeEndian::Little;

        // Borrow a `Cow<[u8]>` to create an `EndianSlice`.
        let borrow_section: &dyn for<'b> Fn(
            &'b std::borrow::Cow<[u8]>,
            ) -> gimli::EndianSlice<'b, gimli::RunTimeEndian> =
            &|section| gimli::EndianSlice::new(&*section, endian);

        // Create `EndianSlice`s for all of the sections.
        //self.dwarf = Some(DwarfInfo{ bin_data: bin_data, dwarf: dwarf_cow.borrow(&borrow_section) });
        let binding = self.dwarf.dwarf_cow.as_ref().unwrap();
        let dwarf = binding.borrow(&borrow_section);

        // Iterate over the compilation units.
        let mut iter = dwarf.units();
        while let Some(header) = iter.next().unwrap() {
            println!(
                "Line number info for unit at <.debug_info+0x{:x}>",
                header.offset().as_debug_info_offset().unwrap().0
                );
            let unit = dwarf.unit(header).unwrap();

            // Get the line program for the compilation unit.
            if let Some(program) = unit.line_program.clone() {
                let comp_dir = if let Some(ref dir) = unit.comp_dir {
                    std::path::PathBuf::from(dir.to_string_lossy().into_owned())
                } else {
                    std::path::PathBuf::new()
                };

                // Iterate over the line program rows.
                let mut rows = program.rows();
                while let Some((header, row)) = rows.next_row().unwrap() {
                    if row.end_sequence() {
                        // End of sequence indicates a possible gap in addresses.
                        println!("{:x} end-sequence", row.address());
                    } else {
                        // Determine the path. Real applications should cache this for performance.
                        let mut path = std::path::PathBuf::new();
                        if let Some(file) = row.file(header) {
                            path = comp_dir.clone();

                            // The directory index 0 is defined to correspond to the compilation unit directory.
                            if file.directory_index() != 0 {
                                if let Some(dir) = file.directory(header) {
                                    path.push(
                                        dwarf.attr_string(&unit, dir).unwrap().to_string_lossy().as_ref(),
                                        );
                                }
                            }

                            path.push(
                                dwarf
                                .attr_string(&unit, file.path_name()).unwrap()
                                .to_string_lossy()
                                .as_ref(),
                                );
                        }

                        // Determine line/column. DWARF line/column is never 0, so we use that
                        // but other applications may want to display this differently.
                        let line = match row.line() {
                            Some(line) => line.get(),
                            None => 0,
                        };
                        let column = match row.column() {
                            gimli::ColumnType::LeftEdge => 0,
                            gimli::ColumnType::Column(column) => column.get(),
                        };

                        println!("{:x} {}:{}:{}", row.address(), path.display(), line, column);
                    }
                }
            }
        }
        vec![]
    }
}

impl Worker for OfflineDebugInfoWorker<'_> {
    fn work(&mut self) {
        println!("Receiving...");
        let request = self.request_receiver.recv();
        if request.is_err() {
            // TODO: kill the thread. Pottentially give access via it's own control variables
            println!("OfflineDebugInfoWorker should die here!");
            return;
        }
        let request = request.unwrap();

        println!("Request received. Generating offline debug info for {}...", request.path.display());

        let response = Arc::new(SrcFileDebugInfo{ 
            decompiled_src: Self::decompile_src(&request),
            breakable_locations: self.generate_breakable_src_locations(&request),
        });

        println!("Responding with debug info for {}...", request.path.display());
        self.response_sender.send(response);
    }
}

pub struct OfflineDebugInfo {
    // TODO: Multiple threads possibly.
    // TODO: Make and kill the thread along with the debug info for now. 
    // Might be a problem for serialization/deserialization. Problem for future me.
    thread: WorkerThread,
    
    debug_info_request_sender: Sender<Arc<SrcFile>>,
    debug_info_response_receiver: Receiver<Arc<SrcFileDebugInfo>>,

    // Mapping one to one
    // TODO: Loading of source files could also be offloaded to a worker thread. 
    // For cases when the file is loaded from remote source or something..?
    src_files: Vec<Arc<SrcFile>>,
    // TEMP: u64 is a temp here
    debug_info: HashMap<u64, Arc<SrcFileDebugInfo>>,

    // TODO: callchains, all sorts of other info
}

impl Drop for OfflineDebugInfo {
    fn drop(&mut self) {
        self.thread.kill();
    }
}

impl OfflineDebugInfo {
    pub fn new(exec_path: PathBuf) -> Result<Self> {
        let (worker, request_sender, response_receiver) = OfflineDebugInfoWorker::new(exec_path);

        Ok(Self { 
            thread: WorkerThread::new("OfflineDebugInfoThread".to_owned(), Box::new(worker))?,
            debug_info_request_sender: request_sender,
            debug_info_response_receiver: response_receiver,
            src_files: vec![],
            debug_info: HashMap::new(),
        })
    }

    pub fn load_file(&mut self, path: PathBuf, queue_debug_info: bool) -> Result<()> {
        let file = Arc::new(SrcFile::new(path, true)?);
        if queue_debug_info && !self.debug_info.contains_key(&file.simple_hash()) {
            self.debug_info_request_sender.send(Arc::clone(&file));
        }
        self.src_files.push(file);

        Ok(())
    }

    pub fn get_debug_info(&self, file: Arc<SrcFile>, queue_debug_info: bool) -> Option<Arc<SrcFileDebugInfo>> {
        let hash = file.simple_hash();
        if queue_debug_info && !self.debug_info.contains_key(&file.simple_hash()) {
            self.debug_info_request_sender.send(Arc::clone(&file));
        }

        self.debug_info.get(&hash).cloned()
    }
}

struct WorkerThread {
    join_handle: JoinHandle<()>,
    parked: Arc<AtomicBool>,
    kill_signal: Arc<AtomicBool>,
}

impl WorkerThread {
    pub fn new(thread_name: String, mut worker: Box<dyn Worker + Send>) -> Result<Self>
    {
        let parked = Arc::new(AtomicBool::new(false));
        let kill_signal = Arc::new(AtomicBool::new(false));

        let cloned_parked = Arc::clone(&parked);
        let cloned_kill_signal = Arc::clone(&kill_signal);
        let join_handle = Builder::new()
            .name(thread_name)
            .spawn(move || {
                loop {
                    worker.work();

                    while cloned_parked.load(Ordering::Relaxed) {
                        std::thread::park();
                    }
                    if cloned_kill_signal.load(Ordering::Relaxed) {
                        break;
                    }
                }
            })?;

        Ok(WorkerThread{ join_handle: join_handle, parked: parked, kill_signal: kill_signal })
    }

    pub fn wakeup(&mut self) {
        self.parked.store(false, Ordering::Relaxed);
        self.join_handle.thread().unpark();
    }

    pub fn kill(&mut self) {
        self.wakeup();
        self.kill_signal.store(true, Ordering::Relaxed);
    }

    pub fn working(&self) -> bool {
        self.alive() && !self.parked.load(Ordering::Relaxed) && !self.kill_signal.load(Ordering::Relaxed)
    }

    pub fn alive(&self) -> bool {
        !self.join_handle.is_finished()
    }
}
