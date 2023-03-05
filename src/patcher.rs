use nix::sys::{ ptrace, wait::waitpid };
use nix::unistd::Pid;
use core::ffi::c_void;

// For now just bps from addresses
pub trait Patcher {
    fn inject_breakpoints(&mut self, breakpoints: &Vec<u64>) -> Result<(), ()>;
    fn disable_breakpoints(&mut self, breakpoints: &Vec<u64>) -> Result<(), ()>;
    fn enable_breakpoints(&mut self, breakpoints: &Vec<u64>) -> Result<(), ()>;

    fn cont(&mut self, addr: u64) -> Result<(), ()>;
}

struct Patch {
    addr: u64, 
    original_instruction: i64, // TEMP: change to u8
    new_instruction: i64,
    active: bool,
}

const x86_sigtrap: i64 = 0xCC; // TEMP: change to u8
pub struct LocalPatcher {
    pid: Pid,

    patches: Vec<Patch>,
}

impl LocalPatcher {
    pub fn new(pid: Pid) -> impl Patcher {
        LocalPatcher{ pid: pid, patches: vec![] }
    }
}

impl Patcher for LocalPatcher {
    fn inject_breakpoints(&mut self, breakpoints: &Vec<u64>) -> Result<(), ()> {
        // TODO: check if we already have such BP
        for addr in breakpoints {
            let mut patch = Patch { addr: *addr, original_instruction: 0, new_instruction: 0, active: false };
            unsafe {
                println!("Adding BP -- pid in patcher: {}, addr: {:#04x}", self.pid, *addr);
                patch.original_instruction = ptrace::read(self.pid, *addr as *mut c_void).expect("Should not fail");
                patch.new_instruction = (patch.original_instruction & !(0xFF as i64)) | x86_sigtrap;
                patch.active = true;

                ptrace::write(self.pid, *addr as *mut c_void, patch.new_instruction as *mut c_void);
            }
            
            self.patches.push(patch);
        }

        Ok(())
    }

    fn cont(&mut self, addr: u64) -> Result<(), ()> {
        let mut regs = ptrace::getregs(self.pid).unwrap();
        if regs.rip - 1 != addr {
            return Err(());
        }

        self.disable_breakpoints(&vec![addr]);

        regs = ptrace::getregs(self.pid).unwrap();
        regs.rip = addr;
        ptrace::setregs(self.pid, regs);

        ptrace::step(self.pid, None).unwrap();
        waitpid(self.pid, None); // NOTE: Bad bad bad bad, will freeze the debugger, pass in a closure

        self.enable_breakpoints(&vec![addr]);
        ptrace::cont(self.pid, None); // NOTE: also should not be here

        Ok(())
    }

    fn disable_breakpoints(&mut self, breakpoints: &Vec<u64>) -> Result<(), ()> {
        for addr in breakpoints {
            for patch in &mut self.patches {
                if *addr != patch.addr {
                    continue;
                }

                if !patch.active {
                    break;
                }

                unsafe {
                    ptrace::write(self.pid, *addr as *mut c_void, patch.original_instruction as *mut c_void);
                }
                patch.active = false;
                return Ok(());
            }
        }
                               
        Err(())
    }

    fn enable_breakpoints(&mut self, breakpoints: &Vec<u64>) -> Result<(), ()> {
        for addr in breakpoints {
            for patch in &mut self.patches {
                if *addr != patch.addr {
                    continue;
                }

                if patch.active {
                    break;
                }

                unsafe {
                    ptrace::write(self.pid, *addr as *mut c_void, patch.new_instruction as *mut c_void);
                }
                patch.active = true;
                return Ok(());
            }
        }
                               
        Err(())
    }
}
