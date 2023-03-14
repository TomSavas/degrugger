use std::path::PathBuf;

use gimli::*;
use object::Object;
use object::ObjectSection;

use crate::insertpoint::BreakPoint;

use crate::src_file::SrcFile;
use crate::patcher::{Patcher, LocalPatcher};

use std::thread::JoinHandle;

use std::sync::Arc;
use std::sync::atomic::{ AtomicBool, Ordering };

use std::sync::mpsc::{ channel, Sender, Receiver };

use std::result::Result;
use nix::sys::wait::WaitStatus;

use linux_personality::{personality, ADDR_NO_RANDOMIZE};
use nix::sys::{ptrace, wait::waitpid};
use nix::unistd::{fork, ForkResult, Pid};
use nix::errno::Errno;

use std::os::unix::process::CommandExt;
use std::process::{ exit, Command };

use std::collections::{ HashSet, HashMap };

struct RunThread {
    pub join_handle: JoinHandle<()>,
    rx: Receiver<Result<WaitStatus, Errno>>,
}

impl RunThread {
    fn new(pid: Pid, parked: Arc<AtomicBool>, should_die: Arc<AtomicBool>) -> Self {
        let (tx, rx) = channel();

        let join_handle = std::thread::spawn(move || {
            let mut res = waitpid(pid, None);
            while res.is_ok() {
                if let Ok(nix::sys::wait::WaitStatus::Exited(..)) = res {
                    // TODO: place death signal in the channel
                    println!("Child exited, run thread died!");
                    break;
                }

                // Before sending off the event so that we don't get deadlocked once
                // we pick this message up by setting parked to false
                parked.store(true, Ordering::Relaxed);
                tx.send(res).expect("Let's assume doesn't fail for now");

                while parked.load(Ordering::Relaxed) {
                    std::thread::park();
                }

                if should_die.load(Ordering::Relaxed) {
                    println!("Seppuku by run thread!");
                    return;
                }

                res = waitpid(pid, None);
            }
        });

        RunThread { join_handle: join_handle, rx: rx }
    }
}

use nix::libc::user_regs_struct as UserRegsStruct;

pub struct DebugeeState {
    pub regs: UserRegsStruct,

    pub addr: i64,
    pub file: String,
    pub line: usize,
    pub col: usize,
}

pub struct RuntimeAddr(u64);
pub struct RuntimeBreakpoint{}

pub struct Run {
    pub debugee_pid: Pid,
    pub debugee_patcher: Box<dyn Patcher>,

    run_thread_parked: Arc<AtomicBool>,
    pub run_thread_should_die: Arc<AtomicBool>,
    run_thread: RunThread,

    pub debugee_event: Option<Result<WaitStatus, Errno>>,
    pub debugee_state: Option<DebugeeState>,

    pub breakpoints: HashMap<RuntimeAddr, RuntimeBreakpoint>,
}

impl Run {
    pub fn new(pid: Pid) -> Self {
        let run_thread_parked = Arc::new(AtomicBool::new(false));
        let run_thread_should_die = Arc::new(AtomicBool::new(false));
        Run { 
            debugee_pid: pid,
            debugee_patcher: Box::new(LocalPatcher::new(pid)),
            run_thread_parked: Arc::clone(&run_thread_parked),
            run_thread_should_die: Arc::clone(&run_thread_should_die),
            run_thread: RunThread::new(pid, Arc::clone(&run_thread_parked), Arc::clone(&run_thread_should_die)),
            debugee_event: None,
            debugee_state: None,
            breakpoints: HashMap::new(),
        }
    }

    pub fn sync_bp_state(&mut self, bps: Vec<BreakPoint<'_>>) {

    }

    pub fn poll_debugee_state(&mut self, block: bool) {
        let msg = if block {
            match self.run_thread.rx.recv() {
                Ok(m) => Ok(m),
                Err(_) => Err(std::sync::mpsc::TryRecvError::Disconnected),
            }
        } else {
            self.run_thread.rx.try_recv()
        };

        match msg {
            Ok(res) => {
                //println!("Signal: {:?}", res);
                self.debugee_event = Some(res);

                let regs = ptrace::getregs(self.debugee_pid).expect("Getting registers failed");
                self.debugee_state = Some(DebugeeState{
                    regs: regs,
                    addr: 0,
                    file: "".to_owned(),
                    line: 0,
                    col: 0,
                });
                // TODO: generate state here
            },
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                println!("Thread died! Killing run");
                // TODO: fuck is this horrible. Return a custom event probably
                //self.debugee_event = Some(Ok(nix::sys::wait::WaitStatus::Exited(self.debugee_pid, -1)));
                self.debugee_event = Some(Err(Errno::EOWNERDEAD));
            },
            _ => {}, // Ignore empty channel issues
        }
    }

    pub fn cont(&mut self) {
        if !self.run_thread_parked.load(Ordering::Relaxed) {
            return;
        }

        self.run_thread_parked.store(false, Ordering::Relaxed);
        self.run_thread.join_handle.thread().unpark();
        
        // TODO: This can get fucked by some timing issues
        // ~~e.g. if this is called before the watcher thread parks itself~~
        // potentially?

        ptrace::cont(self.debugee_pid, None);
    }

    pub fn running(&self) -> bool {
        !self.run_thread_should_die.load(Ordering::Relaxed) && !self.run_thread_parked.load(Ordering::Relaxed) && !self.run_thread.join_handle.is_finished()
    }

    pub fn kill(&mut self) {
        self.run_thread_should_die.store(true, Ordering::Relaxed);
        self.run_thread_parked.store(false, Ordering::Relaxed);
        self.run_thread.join_handle.thread().unpark();
    }
}

#[derive(Debug)]
pub struct Function {
    pub low_pc: u64,
    pub high_pc: u64,
    pub name: String,
}

pub struct Session<'a> {
    exec_path: PathBuf,

    saved_on_disk: bool, // store some save metadata
    saved_path: Option<PathBuf>,

    //debug_info: DebugInfo,
    //binary: BinaryFile,

    //insertpoints: Vec<Box<dyn InsertPoint>>,
    //insertpoint_groups: Vec<InsertPointGroup>,
    pub breakpoints: Vec<BreakPoint<'a>>,

    pub open_files: Vec<SrcFile>,
    pub function_ranges: Vec<Function>, // TEMP: i just wanna go to sleep 
        
    pub active_run: Option<Run>,
}

impl<'a> Session<'a> {
    pub fn add_breakpoint(bp: BreakPoint<'_>) {
        
    }

    pub fn reconcile_bp_state_with_run() {
        
    }

    pub fn new(path_str: String) -> std::result::Result<Session<'a>, ()> {
        let mut session = Session{ exec_path: PathBuf::from(path_str), saved_on_disk: false, saved_path: None, breakpoints: vec![], open_files: vec![], function_ranges: vec![], active_run: None };

        let path = session.exec_path.as_path();
        if !path.exists() || !path.is_file() {
            return Err(());
        }

        // TEMP: load all files occuring and open them up
        let endian = gimli::RunTimeEndian::Little;

        let bin_data = std::fs::read(session.exec_path.as_path()).unwrap();
        let object_owned = object::File::parse(&*bin_data).unwrap();
        let object = &object_owned;

        let mut file_to_line_to_addr: HashMap<String, HashMap<usize, u64>> = HashMap::new();
        let mut file_to_addr_to_line: HashMap<String, HashMap<u64, usize>> = HashMap::new();

        let load_section = |id: gimli::SectionId| -> std::result::Result<std::borrow::Cow<[u8]>, gimli::Error> {
            match object.section_by_name(id.name()) {
                Some(ref section) => Ok(section
                                        .uncompressed_data()
                                        .unwrap_or(std::borrow::Cow::Borrowed(&[][..]))),
                None => Ok(std::borrow::Cow::Borrowed(&[][..])),
            }
        };

        // Load all of the sections.
        let dwarf_cow = gimli::Dwarf::load(&load_section).unwrap();

        // Borrow a `Cow<[u8]>` to create an `EndianSlice`.
        let borrow_section: &dyn for<'b> Fn(
            &'b std::borrow::Cow<[u8]>,
            ) -> gimli::EndianSlice<'b, gimli::RunTimeEndian> =
            &|section| gimli::EndianSlice::new(&*section, endian);

        // Create `EndianSlice`s for all of the sections.
        let dwarf = dwarf_cow.borrow(&borrow_section);

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

                        if !file_to_line_to_addr.contains_key(&path.display().to_string()) {
                            file_to_line_to_addr.insert(path.clone().display().to_string(), HashMap::new());
                        }
                        file_to_line_to_addr.get_mut(&path.display().to_string()).unwrap().insert(line as usize, row.address() as u64);

                        if !file_to_addr_to_line.contains_key(&path.display().to_string()) {
                            file_to_addr_to_line.insert(path.clone().display().to_string(), HashMap::new());
                        }
                        file_to_addr_to_line.get_mut(&path.display().to_string()).unwrap().insert(row.address() as u64, line as usize);
                    }
                }

                let mut iter = dwarf.units();
                while let Some(header) = iter.next().unwrap() {
                    println!(
                        "Unit at <.debug_info+0x{:x}>",
                        header.offset().as_debug_info_offset().unwrap().0
                        );
                    let unit = dwarf.unit(header).unwrap();

                    // Iterate over the Debugging Information Entries (DIEs) in the unit.
                    let mut depth = 0;
                    let mut entries = unit.entries();
                    while let Some((delta_depth, entry)) = entries.next_dfs().unwrap() {
                        if entry.tag() != gimli::constants::DwTag(0x2e) {
                            continue;
                        }

                        depth += delta_depth;
                        println!("<{}><{:x}> {}", depth, entry.offset().0, entry.tag());

                        // Iterate over the attributes in the DIE.
                        let mut attrs = entry.attrs();
                        let mut func = Function { low_pc: 0, high_pc: 0, name: "".to_owned() };
                        while let Some(attr) = attrs.next().unwrap() {
                            match attr.name() {
                                DW_AT_low_pc => {
                                    func.low_pc = dwarf.attr_address(&unit, attr.value()).unwrap().unwrap();
                                },
                                DW_AT_high_pc => {
                                    func.high_pc = attr.udata_value().unwrap() + func.low_pc;
                                },
                                DW_AT_name => {
                                    func.name = dwarf.attr_string(&unit, attr.value()).unwrap().to_string().unwrap().to_string();
                                },
                                //DW_AT_decl_file => {
                                //    let name = dwarf.attr_string(&unit, attr.value()).unwrap().to_string().unwrap();
                                //    println!("file: {}", name);
                                //}, 
                                _ => {},
                            }
                        }
                        println!("{:?}", func);
                        session.function_ranges.push(func);
                    }
                }
            }
        }

        // Yea, well fuck all of the above, just walk it once and cache most of the useful data
        println!("Source files:");
        for (file, line_to_addr) in file_to_line_to_addr.into_iter() {
            println!("\t{}", file);
            for (line, addr) in line_to_addr.iter() {
                println!("\t\t{} at {:x}", line, addr);
            }

            let src_file = SrcFile::new(std::path::PathBuf::from(&file), true);
            if let Ok(mut f) = src_file {
                f.line_to_addr = line_to_addr;
                f.addr_to_line = file_to_addr_to_line.get(&file).unwrap().clone();
                session.open_files.push(f);
            }
        }

        Ok(session)
    }

    fn launch_child(path: &str) {
        println!("Launching {path}");

        ptrace::traceme().expect("Failed to TRACEME");

        match personality(ADDR_NO_RANDOMIZE) {
            Ok(p) => println!("Disabled ASLR. Previous personality: {:?}", p),
            Err(e) => println!("Failed disabling ASLR: {:?}", e),
        }

        Command::new(path).exec();
        exit(0);
    }

    pub fn start_run(&mut self) -> std::result::Result<&Run, Errno> {
        let fork_res = unsafe { fork() }?;
        match fork_res {
            ForkResult::Parent{ child, .. } => {
                println!("Child pid: {child}");

                {
                    let mut run = Run::new(child);

                    // At this point the debugee has launched and should have SIGTRAPped
                    run.poll_debugee_state(true);
                    match run.debugee_event {
                        Some(Ok(nix::sys::wait::WaitStatus::Stopped(_, nix::sys::signal::Signal::SIGTRAP))) => {
                            // TODO: fix this bullshit
                            let addresses: Vec<u64> = self.breakpoints.iter().map(|bp| {
                                println!("BP addr: {:x}, line: {}", bp.point.addr, bp.point.line_number);
                                bp.point.addr + 0x555555555040 - 0x1040
                            }).collect();
                            run.debugee_patcher.inject_breakpoints(&addresses);
                        },
                        _ => { panic!("Errrm, something went wrong..."); }
                    }
                    run.cont();
                    self.active_run = Some(run);
                }
            },
            ForkResult::Child => {
                Self::launch_child(self.exec_path.to_str().unwrap());
            },
        };

        Ok(&self.active_run.as_ref().unwrap())
    }
}
