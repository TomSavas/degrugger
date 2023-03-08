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
use std::process::{exit, Command};

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

pub struct Run {
    pub debugee_pid: Pid,
    pub debugee_patcher: Box<dyn Patcher>,

    run_thread_parked: Arc<AtomicBool>,
    pub run_thread_should_die: Arc<AtomicBool>,
    run_thread: RunThread,
    pub debugee_event: Option<Result<WaitStatus, Errno>>,
}

impl Run {
    pub fn new(pid: Pid) -> Self {
        let mut run_thread_parked = Arc::new(AtomicBool::new(false));
        let mut run_thread_should_die = Arc::new(AtomicBool::new(false));
        Run { 
            debugee_pid: pid,
            debugee_patcher: Box::new(LocalPatcher::new(pid)),
            run_thread_parked: Arc::clone(&run_thread_parked),
            run_thread_should_die: Arc::clone(&run_thread_should_die),
            run_thread: RunThread::new(pid, Arc::clone(&run_thread_parked), Arc::clone(&run_thread_should_die)),
            debugee_event: None
        }
    }

    pub fn poll_debugee_events(&mut self, block: bool) {
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
                println!("Signal: {:?}", res);
                self.debugee_event = Some(res);
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

    pub fn kill(&mut self) {
        self.run_thread_should_die.store(true, Ordering::Relaxed);
        self.run_thread_parked.store(false, Ordering::Relaxed);
        self.run_thread.join_handle.thread().unpark();
    }
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
        
    pub active_run: Option<Run>,
}

impl<'a> Session<'a> {
    pub fn new(path_str: String) -> std::result::Result<Session<'a>, ()> {
        let mut session = Session{ exec_path: PathBuf::from(path_str), saved_on_disk: false, saved_path: None, breakpoints: vec![], open_files: vec![], active_run: None };

        let path = session.exec_path.as_path();
        if !path.exists() || !path.is_file() {
            return Err(());
        }

        // TEMP: load all files occuring and open them up
        let endian = gimli::RunTimeEndian::Little;

        let bin_data = std::fs::read(session.exec_path.as_path()).unwrap();
        let object_owned = object::File::parse(&*bin_data).unwrap();
        let object = &object_owned;

        let mut files: HashMap<String, HashMap<usize, u64>> = HashMap::new();

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

                        if !files.contains_key(&path.display().to_string()) {
                            files.insert(path.clone().display().to_string(), HashMap::new());
                        }
                        files.get_mut(&path.display().to_string()).unwrap().insert(line as usize, row.address() as u64);
                    }
                }
            }
        }

        println!("Source files:");
        for (file, bps) in files.into_iter() {
            println!("\t{}", file);
            for (line, addr) in bps.iter() {
                println!("\t\t{} at {:x}", line, addr);
            }

            let src_file = SrcFile::new(std::path::PathBuf::from(file), true);
            if let Ok(mut f) = src_file {
                f.line_to_addr = bps;
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
                    run.poll_debugee_events(true);
                    match run.debugee_event {
                        Some(Ok(nix::sys::wait::WaitStatus::Stopped(_, SIGTRAP))) => {
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
