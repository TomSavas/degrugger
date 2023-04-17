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

use crate::OfflineDebugInfo;

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

    pub addr: u64,
    pub file: String,
    pub line: Option<usize>,
    pub col: Option<usize>,
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
                    addr: regs.rip,
                    file: "".to_owned(),
                    line: None,
                    col: None,
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

    pub debug_info: OfflineDebugInfo,

    //debug_info: DebugInfo,
    //binary: BinaryFile,

    //insertpoints: Vec<Box<dyn InsertPoint>>,
    //insertpoint_groups: Vec<InsertPointGroup>,
    pub breakpoints: Vec<BreakPoint<'a>>,

    pub active_run: Option<Run>,
}

impl<'a> Session<'a> {
    pub fn sync_workers(&mut self ) {
        self.debug_info.sync_debug_info();
    }

    pub fn add_breakpoint(bp: BreakPoint<'_>) {
        
    }

    pub fn reconcile_bp_state_with_run() {
        
    }

    pub fn new(path_str: String, auto_load_src_root: Option<String>) -> std::result::Result<Session<'a>, ()> {
        let exec_path = PathBuf::from(path_str);
        let mut session = Session{ exec_path: exec_path.clone(), saved_on_disk: false, saved_path: None, debug_info: OfflineDebugInfo::new(exec_path.clone(), auto_load_src_root).unwrap(), breakpoints: vec![], active_run: None };

        let path = session.exec_path.as_path();
        if !path.exists() || !path.is_file() {
            return Err(());
        }

        session.debug_info.load_exec(exec_path);
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

//enum DebugeeEvent {
//    Status(WaitStatus),
//    Error(Errno),
//    None,
//}
//
//pub struct RuntimeAddr(usize);
//
//struct DebugeeWatcherThread {
//
//}
//
//struct Callsite {
//    addr: RuntimeAddr,
//
//    filename: String,
//    function: String,
//
//    line: usize, 
//    col: usize,
//}   
//
//struct StoppedRuntimeInfo {
//    // TODO: for now single-thread focused
//    regs: UserRegsStruct,
//    addr: RuntimeAddr,
//
//    callstack: Vec<Callsite>,
//}
//
//struct RuntimeDebugInfo {
//    stopped_info: Option<StoppedRuntimeInfo>,
//    // TODO: for now remap every address. Not 100% on how relocation and mixing works. Most likely
//    // does so in blocks.
//    address_remapping_offline_to_runtime: HashMap<OfflineAddr, RuntimeAddr>,
//    address_remapping_runtime_to_offline: HashMap<RuntimeAddr, OfflineAddr>,
//}
//
//struct RuntimeDebugInfoThread {
//}
//
//pub struct Run {
//    debugee_pid: Pid,
//    debugee_patcher: Box<dyn Patcher>,
//
//    debugee_watcher_thread: DebugeeWatcherThread,
//    debugee_events: VecDeque<DebugeeEvent>,
//
//    debug_info_thread: RuntimeDebugInfoThread, // Actually the same thread as debugee watcher thread
//    debug_info: RuntimeDebugInfo,
//}
//
//struct FrontendBreakpoint {
//    file: &DebugSrcFile,
//    location: BreakableLocation,
//
//    enabled: bool,
//}
//
//pub struct Session<'a> {
//    exec_path: BinFile,
//
//    debug_info: OfflineDebugInfo,
//
//    // TODO: do session saving
//
//    breakpoints: Vec<FrontendBreakpoint>,
//    open_files: Vec<&DebugSrcFile>,
//    active_run: Option<Run>,
//}
