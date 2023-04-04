use std::collections::HashMap;
use std::marker::Send;
use std::io::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{ AtomicBool, Ordering };
use std::sync::mpsc::{ channel, Receiver, Sender, TryRecvError };
use std::thread::{ Builder, JoinHandle };

use iced_x86::{ Decoder, DecoderOptions, Formatter, Instruction, NasmFormatter, FastFormatter };

use crate::SrcFile;

type OfflineAddr = u64;

#[derive(Clone)]
pub struct ThinOfflineDebugInfo {
    pub decompiled_src: Option<Arc<DecompiledSrc>>,
    pub src_file_info: HashMap<u64, Arc<SrcFileDebugInfo>>,
    pub all_subprograms: Arc<Vec<Subprogram>>,
}

impl ThinOfflineDebugInfo {
    fn empty() -> ThinOfflineDebugInfo {
        ThinOfflineDebugInfo{ decompiled_src: None, src_file_info: HashMap::new(), all_subprograms: Arc::new(vec![])}
    }
}

#[derive(Debug)]
pub struct DecompiledSrc {
    pub decompiled_src: Vec<String>,
    pub addresses: Vec<OfflineAddr>,
}

#[derive(Debug)]
#[derive(Clone)]
pub struct Subprogram {
    pub name: String,
    pub low_addr: OfflineAddr,
    pub high_addr: OfflineAddr,
    pub src_file_hash: u64,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug)]
#[derive(Clone)]
pub struct BreakableSrcLocation {
    pub addr: OfflineAddr,
    pub src_line: usize,
    pub src_col: usize,
}

#[derive(Debug)]
pub struct SrcFileDebugInfo {
    pub src_file_hash: u64,
    pub breakable_locations: Vec<BreakableSrcLocation>,
    // Duplicated subarray of all_subprograms. 
    // TODO: unduplicate once I figure out how to not lose my mind when dealing with lifetimes
    pub subprograms: Vec<Subprogram>
}

trait Worker {
    fn work(&mut self);
}

struct OfflineDebugInfoWorker {
    request_receiver: Receiver<Arc<SrcFile>>,
    response_sender: Sender<ThinOfflineDebugInfo>,

    debug_info: ThinOfflineDebugInfo,

    exec_path: PathBuf,
    bin_data: Vec<u8>,
}

// TEMP
use gimli::*;
use object::Object;
use object::ObjectSection;

impl OfflineDebugInfoWorker {
    pub fn new(exec_path: PathBuf) -> (Self, Sender<Arc<SrcFile>>, Receiver<ThinOfflineDebugInfo>) {
        let (request_sender, request_receiver) = channel();
        let (response_sender, response_receiver) = channel();

        (Self{ request_receiver: request_receiver, response_sender: response_sender, debug_info: ThinOfflineDebugInfo::empty(), exec_path: exec_path, bin_data: vec![] }, request_sender, response_receiver)
    }

    fn gather_dwarf_info(&mut self) {
        println!("Analysing dwarf...");
        // TODO: move more of the parsing code from generate_breakable_src_locations to here 
        // once I figure out what black magic are lifetimes
        self.bin_data = std::fs::read(self.exec_path.clone()).unwrap();
    }

    fn decompile_src(bin_data: &Vec<u8>) -> Arc<DecompiledSrc> {
        let obj_file = object::File::parse(&**bin_data).unwrap();
        let text_section = obj_file.section_by_name(".text").unwrap();

        let mut decompiled_src = DecompiledSrc{ decompiled_src: vec![], addresses: vec![]/*, src_to_decompiled_mapping: HashMap::new()*/ };

        let mut decoder = Decoder::new(64, text_section.data().unwrap(), DecoderOptions::NONE);
        decoder.set_ip(text_section.address());
        let mut formatter = FastFormatter::new();

        let mut output = String::new();
        let mut instruction = Instruction::default();

        while decoder.can_decode() {
            decoder.decode_out(&mut instruction);
            output.clear();
            formatter.format(&instruction, &mut output);

            let mut hex_instruction = "".to_owned();
            let start_index = (instruction.ip() - 0) as usize;
            //for b in &bin_data[start_index..start_index + instruction.len()] {
            //    hex_instruction = format!("{}{:02x}", hex_instruction, b);
            //}
            // TODO: remove the address from here
            //let decompiled_asm = format!("{:016x} {:<20} {}", instruction.ip(), hex_instruction, output);
            let decompiled_asm = format!("{:016x} {}", instruction.ip(), output);
            decompiled_src.decompiled_src.push(decompiled_asm);
            decompiled_src.addresses.push(instruction.ip());
        }

        Arc::new(decompiled_src)
    }

    // TODO: this is catastrophically bad -- we shouldn't be reparsing it for every file, etc.
    // This should be done once and kept in memory while we need it. But lifetimes are an absolute PITA >:C
    fn generate_breakable_src_locations_and_subprograms(&mut self, src_file: &SrcFile) -> (Vec<BreakableSrcLocation>, Vec<Subprogram>) {
        let mut breakable_src_locs = vec![];
        let mut subprograms = vec![];

        let endian = gimli::RunTimeEndian::Little;
        let object_owned = object::File::parse(&*self.bin_data).unwrap();
        let object = &object_owned;

        let load_section = |id: gimli::SectionId| -> std::result::Result<std::borrow::Cow<[u8]>, gimli::Error> {
            match object.section_by_name(id.name()) {
                Some(ref section) => Ok(section
                                        .uncompressed_data()
                                        .unwrap_or(std::borrow::Cow::Borrowed(&[][..]))),
                None => Ok(std::borrow::Cow::Borrowed(&[][..])),
            }
        };

        // Borrow a `Cow<[u8]>` to create an `EndianSlice`.
        let borrow_section: &dyn for<'b> Fn(
            &'b std::borrow::Cow<[u8]>,
            ) -> gimli::EndianSlice<'b, gimli::RunTimeEndian> =
            &|section| gimli::EndianSlice::new(&*section, endian);


        // Create `EndianSlice`s for all of the sections.
        let dwarf_cow = gimli::Dwarf::load(&load_section).unwrap();
        let dwarf = dwarf_cow.borrow(&borrow_section);

        // Iterate over the compilation units.
        let mut iter = dwarf.units();
        while let Some(header) = iter.next().unwrap() {
            //println!(
            //    "Line number info for unit at <.debug_info+0x{:x}>",
            //    header.offset().as_debug_info_offset().unwrap().0
            //    );
            let unit = dwarf.unit(header).unwrap();

            let program = unit.line_program.clone();
            let program = match program {
                Some(p) => p,
                None => { continue; }
            };

            // Get the line program for the compilation unit.
            let comp_dir = if let Some(ref dir) = unit.comp_dir {
                std::path::PathBuf::from(dir.to_string_lossy().into_owned())
            } else {
                std::path::PathBuf::new()
            };
            let mut path = std::path::PathBuf::new();

            // Iterate over the line program rows.
            let mut rows = program.rows();
            while let Some((header, row)) = rows.next_row().unwrap() {
                if row.end_sequence() {
                    // End of sequence indicates a possible gap in addresses.
                    //println!("{:x} end-sequence", row.address());
                    continue;
                }

                if let Some(file) = row.file(header) {
                    if path.display().to_string().len() == 0 {
                        path = comp_dir.clone();

                        // The directory index 0 is defined to correspond to the compilation unit directory.
                        if file.directory_index() != 0 {
                            if let Some(dir) = file.directory(header) {
                                path.push(dwarf.attr_string(&unit, dir).unwrap().to_string_lossy().as_ref());
                            }
                        }
                        path.push(dwarf.attr_string(&unit, file.path_name()).unwrap().to_string_lossy().as_ref());
                    }
                }
                if path != src_file.path {
                    continue;
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

                //println!("{:x} {}:{}:{}", row.address(), path.display(), line, column);
                breakable_src_locs.push(BreakableSrcLocation{ addr: row.address(), src_line: line as usize, src_col: column as usize });
            }


            // Iterate over the Debugging Information Entries (DIEs) in the unit.
            let mut depth = 0;
            let mut entries = unit.entries();
            while let Some((delta_depth, entry)) = entries.next_dfs().unwrap() {
                //if entry.tag() != gimli::constants::DwTag(0x2e) {
                if entry.tag() != DW_TAG_subprogram {
                    continue;
                }

                depth += delta_depth;
                //println!("<{}><{:x}> {}", depth, entry.offset().0, entry.tag());

                // Iterate over the attributes in the DIE.
                let mut attrs = entry.attrs();
                let mut subprogram = Subprogram{ name: "".to_owned(), low_addr: 0, high_addr: 0, src_file_hash: src_file.simple_hash(), start_line: 0, end_line: 0 };
                let mut is_inlined = false;
                let mut decl_file = "".to_owned();
                while let Some(attr) = attrs.next().unwrap() {
                    match attr.name() {
                        DW_AT_low_pc => {
                            subprogram.low_addr = dwarf.attr_address(&unit, attr.value()).unwrap().unwrap();
                            //println!("{:?}", attr.value());
                            //subprogram.low_addr = dwarf.address(&unit, attr.value()).unwrap().unwrap();
                        },
                        DW_AT_high_pc => {
                            subprogram.high_addr = attr.udata_value().unwrap();
                        },
                        DW_AT_name => {
                            subprogram.name = dwarf.attr_string(&unit, attr.value()).unwrap().to_string().unwrap().to_string();
                        },
                        DW_AT_decl_file => {
                            //let name = dwarf.attr_string(&unit, attr.value()).unwrap().to_string().unwrap();
                            dump_file_index(&mut decl_file, attr.udata_value().unwrap(), &unit, &dwarf);
                            //println!("file: {}", n);
                        }, 
                        DW_AT_inline => {
                            is_inlined = true;
                        },
                        _ => {},
                    }
                }

                if std::path::PathBuf::from(decl_file) != src_file.path {
                    continue;
                }
                //println!("{} {} - {} {:x}-{:x}", subprogram.name, subprogram.low_addr, subprogram.high_addr, subprogram.low_addr, subprogram.high_addr);

                subprogram.high_addr += subprogram.low_addr;

                if subprogram.high_addr == subprogram.low_addr || is_inlined {
                    continue;
                }

                for breakable_location in &breakable_src_locs {
                    if breakable_location.addr == subprogram.low_addr {
                        subprogram.start_line = breakable_location.src_line;
                    }
                    if breakable_location.addr <= subprogram.high_addr {
                        subprogram.end_line = breakable_location.src_line;
                    }
                }

                // TODO: well if there's a single line function...
                if subprogram.start_line == subprogram.end_line {
                    continue;
                }

                subprograms.push(subprogram);
            }
        }

        (breakable_src_locs, subprograms)
    }
}

fn dump_file_index<R: Reader>(
        w: &mut String,
        file_index: u64,
        unit: &gimli::Unit<R>,
        dwarf: &gimli::Dwarf<R>,
        ) -> Result<()> {
    if file_index == 0 && unit.header.version() <= 4 {
        return Ok(());
    }
    let header = match unit.line_program {
        Some(ref program) => program.header(),
        None => return Ok(()),
    };
    let file = match header.file(file_index) {
        Some(file) => file,
            None => {
                //writeln!(w, "Unable to get header for file {}", file_index)?;
                return Ok(());
            }
    };
    //write!(w, " ")?;
    if let Some(directory) = file.directory(header) {
        let directory = dwarf.attr_string(unit, directory).unwrap();
        let directory = directory.to_string_lossy().unwrap();
        if file.directory_index() != 0 && !directory.starts_with('/') {
            if let Some(ref comp_dir) = unit.comp_dir {
                //write!(w, "{}/", comp_dir.to_string_lossy()?,)?;
                *w = format!("{}{}/", w, comp_dir.to_string_lossy().unwrap());
            }
        }
        //write!(w, "{}/", directory)?;
        *w = format!("{}{}/", w, &directory);
    }
    //write!(
    //        w,
    //        "{}",
    //        dwarf
    //        .attr_string(unit, file.path_name())?
    //        .to_string_lossy()?
    //      )?;
    *w = format!(
            "{}{}",
            w,
            dwarf
            .attr_string(unit, file.path_name()).unwrap()
            .to_string_lossy().unwrap()
          );
    Ok(())
}

impl Worker for OfflineDebugInfoWorker {
    fn work(&mut self) {
        let mut new_debug_info = self.debug_info.clone();

        if self.bin_data.len() == 0 {
            println!("Reading binary...");   
            self.gather_dwarf_info();
        }

        // TODO: don't do this on startup. Rather wait for specific request
        if self.debug_info.decompiled_src.is_none() {
            println!("Decompiling binary...");   
            new_debug_info.decompiled_src = Some(Self::decompile_src(&self.bin_data));
        }

        println!("Receiving...");
        let request = self.request_receiver.recv();
        if request.is_err() {
            // TODO: kill the thread. Pottentially give access via it's own control variables
            println!("OfflineDebugInfoWorker should die here!");
            return;
        }
        let request = request.unwrap();

        println!("Request received. Generating offline debug info for {}...", request.path.display());

        let (breakable_locations, mut subprograms) = self.generate_breakable_src_locations_and_subprograms(&request);

        let hash = request.simple_hash();
        new_debug_info.src_file_info.insert(hash, 
            Arc::new(SrcFileDebugInfo{ 
                src_file_hash: request.simple_hash(),
                breakable_locations: breakable_locations,
                subprograms: subprograms.clone(),
            }));
        subprograms.append(&mut (*new_debug_info.all_subprograms).clone());
        new_debug_info.all_subprograms = Arc::new(subprograms);

        println!("Responding with debug info for {}...", request.path.display());
        //println!("Responding...");
        self.debug_info = new_debug_info.clone();
        self.response_sender.send(new_debug_info);
    }
}

pub struct OfflineDebugInfo {
    // TODO: Multiple threads possibly.
    // TODO: Make and kill the thread along with the debug info for now. 
    // Might be a problem for serialization/deserialization. Problem for future me.
    thread: WorkerThread,
    
    debug_info_request_sender: Sender<Arc<SrcFile>>,
    debug_info_response_receiver: Receiver<ThinOfflineDebugInfo>,

    // Mapping one to one
    // TODO: Loading of source files could also be offloaded to a worker thread. 
    // For cases when the file is loaded from remote source or something..?
    // TEMP: u64 is a temp here
    pub src_files: HashMap<u64, Arc<SrcFile>>,
    // TEMP: u64 is a temp here
    //pub debug_info: HashMap<u64, Arc<SrcFileDebugInfo>>,

    pub debug_infoo: ThinOfflineDebugInfo,

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
            src_files: HashMap::new(),
            //debug_info: HashMap::new(),
            debug_infoo: ThinOfflineDebugInfo::empty(),
        })
    }

    pub fn load_file(&mut self, path: PathBuf, queue_debug_info: bool) -> Result<()> {
        let file = Arc::new(SrcFile::new(path, true)?);
        if queue_debug_info && !self.debug_infoo.src_file_info.contains_key(&file.simple_hash()) {
            self.debug_info_request_sender.send(Arc::clone(&file));
        }
        self.src_files.insert(file.simple_hash(), file);

        Ok(())
    }

    pub fn get_debug_info(&self, file: Arc<SrcFile>, queue_debug_info: bool) -> Option<Arc<SrcFileDebugInfo>> {
        let hash = file.simple_hash();
        if queue_debug_info && !self.debug_infoo.src_file_info.contains_key(&file.simple_hash()) {
            self.debug_info_request_sender.send(Arc::clone(&file));
        }

        self.debug_infoo.src_file_info.get(&hash).cloned()
    }

    pub fn sync_debug_info(&mut self) {
        //println!("Syncing");
        let response = match self.debug_info_response_receiver.try_recv() {
            Err(TryRecvError::Empty) => {
                return;
            }, 
            Err(TryRecvError::Disconnected) => {
                println!("OfflineDebugInfoWorker should die here!");
                return;
            }, 
            Ok(r) => r,
        };

        //self.debug_info.insert(response.src_file_hash, response);
        self.debug_infoo = response;
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
