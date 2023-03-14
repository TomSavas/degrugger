use std::path::Path;
use mint::*;

use crate::session::Session;
mod session;

use imgui::*;
use imgui::sys;
use std::ptr::null;

mod support;

use crate::src_file::SrcFile;
mod src_file;

mod insertpoint;
use crate::insertpoint::BreakPoint;
use crate::insertpoint::Point;

mod patcher;

use crate::session::Run;

use crate::session::DebugeeState;
use crate::session::Function; // TEMP
use core::ffi::c_void; // TEMP
use nix::unistd::Pid; // TEMP

use nix::sys::{ptrace, wait::waitpid}; // TEMP

struct DebuggerContext<'a> {
    path_input: String,
    session: Result<Session<'a>, ()>,

    hex_values: bool,
}

fn code_windows(ui: &imgui::Ui, files: &Vec<SrcFile>, state: &Option<DebugeeState>, line_num_str: &Vec<String>, breakpoints: &mut Vec<BreakPoint>) {
    let w = ui.window("Src code").begin();
    if w.is_none() {
        return;
    }
    let w = w.unwrap();

    let t = ui.tab_bar("Code");
    if t.is_none() {
        return;
    }
    let t = t.unwrap();

    for file in files {
        if let Some(tab_item) = ui.tab_item(&file.path.file_name().unwrap().to_str().unwrap()) {
            code_windoww(ui, file, state, &line_num_str, breakpoints);
            tab_item.end();
        }
    }

    t.end();
    w.end();
}

fn code_windoww(ui: &imgui::Ui, file: &SrcFile, state: &Option<DebugeeState>, line_num_str: &Vec<String>, breakpoints: &mut Vec<BreakPoint>) {
    if file.lines.is_none() {
        return;
    }
    let lines = file.lines.as_ref().unwrap();

    let char_height = ui.calc_text_size(&" ")[1];
    let char_width = ui.calc_text_size(&" ")[0];

    let mut line_num = 0;

    let draw_list = ui.get_window_draw_list();
    let start_cursor = ui.cursor_screen_pos();
    let content_size = ui.content_region_max();
    let scroll_x = ui.scroll_x();
    let scroll_y = ui.scroll_y();

    let mut bp_line = 0;
    bp_line = match state {
        Some(s) => {
            let addr = s.regs.rip - (0x555555555040 - 0x1040) - 1;
            match file.addr_to_line.get(&addr) {
                Some(l) => *l,
                None => 0,
            }
        },
        None => 0,
    };

    // Line background
    for line in lines {
        let red = Vector4{ x: 0.1, y: 0.1, z: 0.1, w: 1.0};
        let green = Vector4{ x: 0.2, y: 0.2, z: 0.2, w: 1.0};

        let mut background_color = if line_num % 2 == 0 { red } else { green };
        if bp_line == (line_num + 1) {
            background_color = Vector4{ x: 1.0, y: 0.8, z: 0.0, w: 0.7};
        }

        let mut start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height - scroll_y };
        let end = Vector2{ x: start.x + content_size[0], y: start.y + char_height };
        draw_list.add_rect(start, end, background_color).filled(true).build();

        // Line number
        draw_list.add_text(start, ImColor32::WHITE, &line_num_str[line_num]);
        start.x += ui.calc_text_size(&line_num_str[line_num])[0];

        // Code
        draw_list.add_text(start, ImColor32::WHITE, line);

        ui.dummy(Vector2{ x: end.x - start.x, y: end.y - start.y });

        line_num += 1;
    }

    // Code text

    // Line num - text separator
    let c = Vector4{ x: 0.3, y: 0.3, z: 0.3, w: 1.0};
    let start = Vector2{ x: start_cursor[0] + char_width * 5.0, y: start_cursor[1] - scroll_y};
    let e = Vector2{ x: start.x + char_width, y: start.y + char_height * (line_num as f32) };
    draw_list.add_rect(start, e, c).filled(true).build();

    // BPs
    line_num = 0;
    for _ in lines {
        let start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height - scroll_y };
        let end = Vector2{ x: start.x + char_width * 6.0, y: start.y + char_height };

        let mut enabled = false;
        if let Some(addr) = file.line_to_addr.get(&(line_num + 1)) {
            let mut matching_bp: Option<&mut BreakPoint> = None;
            for bp in breakpoints.iter_mut() {
                if bp.point.addr == *addr {
                    enabled = bp.point.enabled;
                    matching_bp = Some(bp);
                    break;
                }
            }

            if ui.is_mouse_hovering_rect(start, end) && ui.is_mouse_clicked(imgui::MouseButton::Left) {
                if let Some(ref mut bp) = matching_bp {
                    enabled = false;
                    bp.point.enabled = enabled;
                } else {
                    enabled = true;
                    breakpoints.push(BreakPoint::new(Point::new(*addr, (line_num + 1) as u64)));

                    let max_bp_count = breakpoints.len() - 1;
                    matching_bp = Some(&mut breakpoints[max_bp_count]);
                }
            }
        }

        if enabled {
            let r = Vector4{ x: 1.0, y: 0.2, z: 0.2, w: 1.0};
            let c = Vector2{ x: start.x + char_width * 5.5, y: start.y + char_height * 0.5 };
            draw_list.add_circle(c, char_width / 2.0, r).filled(true).build();
        }
        line_num += 1;
    }
}

fn code_window(ui: &imgui::Ui, file: &SrcFile, line_num_str: &Vec<String>, breakpoints: &mut Vec<BreakPoint>) {
    // draw code widget
    let w = ui.window(&file.path.file_name().unwrap().to_str().unwrap())
        .position([0.0, 20.0], imgui::Condition::FirstUseEver)
        .size([800.0, 1000.0], imgui::Condition::FirstUseEver)
        .begin();
    if w.is_none() {
        return;
    }
    let w = w.unwrap();

    if file.lines.is_none() {
        w.end();
        return;
    }
    let lines = file.lines.as_ref().unwrap();

    let char_height = ui.calc_text_size(&" ")[1];
    let char_width = ui.calc_text_size(&" ")[0];

    let mut line_num = 0;

    let draw_list = ui.get_window_draw_list();
    let start_cursor = ui.cursor_screen_pos();
    let content_size = ui.content_region_max();
    let scroll_x = ui.scroll_x();
    let scroll_y = ui.scroll_y();

    // Line background
    // TODO: we need to highlight the current line if it has the current BP in it

    for line in lines {
        let red = Vector4{ x: 0.1, y: 0.1, z: 0.1, w: 1.0};
        let green = Vector4{ x: 0.2, y: 0.2, z: 0.2, w: 1.0};

        let mut start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height - scroll_y };
        let end = Vector2{ x: start.x + content_size[0], y: start.y + char_height };
        draw_list.add_rect(start, end, if line_num % 2 == 0 { red } else { green }).filled(true).build();

        // Line number
        draw_list.add_text(start, ImColor32::WHITE, &line_num_str[line_num]);
        start.x += ui.calc_text_size(&line_num_str[line_num])[0];

        // Code
        draw_list.add_text(start, ImColor32::WHITE, line);

        ui.dummy(Vector2{ x: end.x - start.x, y: end.y - start.y });

        line_num += 1;
    }

    // Code text

    // Line num - text separator
    let c = Vector4{ x: 0.3, y: 0.3, z: 0.3, w: 1.0};
    let start = Vector2{ x: start_cursor[0] + char_width * 5.0, y: start_cursor[1] - scroll_y};
    let e = Vector2{ x: start.x + char_width, y: start.y + char_height * (line_num as f32) };
    draw_list.add_rect(start, e, c).filled(true).build();

    // BPs
    line_num = 0;
    for _ in lines {
        let start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height - scroll_y };
        let end = Vector2{ x: start.x + char_width * 6.0, y: start.y + char_height };

        let mut enabled = false;
        if let Some(addr) = file.line_to_addr.get(&(line_num + 1)) {
            let mut matching_bp: Option<&mut BreakPoint> = None;
            for bp in breakpoints.iter_mut() {
                if bp.point.addr == *addr {
                    enabled = bp.point.enabled;
                    matching_bp = Some(bp);
                    break;
                }
            }

            if ui.is_mouse_hovering_rect(start, end) && ui.is_mouse_clicked(imgui::MouseButton::Left) {
                if let Some(ref mut bp) = matching_bp {
                    enabled = false;
                    bp.point.enabled = enabled;
                } else {
                    enabled = true;
                    breakpoints.push(BreakPoint::new(Point::new(*addr, (line_num + 1) as u64)));

                    let max_bp_count = breakpoints.len() - 1;
                    matching_bp = Some(&mut breakpoints[max_bp_count]);
                }
            }
        }

        if enabled {
            let r = Vector4{ x: 1.0, y: 0.2, z: 0.2, w: 1.0};
            let c = Vector2{ x: start.x + char_width * 5.5, y: start.y + char_height * 0.5 };
            draw_list.add_circle(c, char_width / 2.0, r).filled(true).build();
        }
        line_num += 1;
    }

    w.end();
}

fn main_menu(ui: &imgui::Ui, ctx: &mut DebuggerContext, redock: &mut bool) {
    let main_menu_token = ui.begin_main_menu_bar();
    if main_menu_token.is_none() {
        return;
    }
    let main_menu_token = main_menu_token.unwrap();

    if let Some(file_menu_token) = ui.begin_menu("File") {
        let new_input = ui.input_text("Path", &mut ctx.path_input)
            .flags(InputTextFlags::ENTER_RETURNS_TRUE)
            .build();
        ui.same_line();
        let load_pressed = ui.button("Load");

        if new_input || load_pressed {
            ctx.session = Session::new(ctx.path_input.clone());
        }
        file_menu_token.end();
    }

    if ctx.session.is_err() {
        return;
    }

    if ui.button("Redock") {
        *redock = true;
    }

    let mut session = ctx.session.as_mut().unwrap();

    match &mut session.active_run {
        Some(r) => {
            let stop = ui.button("Stop");
            let restart = ui.button("Restart");

            if stop || restart {
                r.kill();
                session.active_run = None;
            }
            if restart {
                session.start_run();
            }
        },
        None => {
            if ui.button("Run") {
                session.start_run();
            }
        }
    }

    if session.active_run.is_none() {
        return;
    }
    let run = session.active_run.as_mut().unwrap();

    ui.disabled(run.running(), || {
        if ui.button("Continue") {
            let s = ctx.session.as_mut().unwrap();
            if let Some(r) = s.active_run.as_mut() {
                let regs = ptrace::getregs(r.debugee_pid).unwrap();

                for bp in s.breakpoints.iter() {
                    let addr = bp.point.addr + 0x555555555040 - 0x1040;
                    if addr == (regs.rip - 1) {
                        r.debugee_patcher.cont(addr);
                        break;
                    }
                }
                r.cont();
            }
        }
    });

    main_menu_token.end();
}

fn reg_row(ui: &imgui::Ui, hex_values: bool, reg: &str, value: nix::libc::c_ulonglong) {
    ui.table_next_column();
    ui.text(reg);
    ui.table_next_column();
    if hex_values {
        ui.text(format!("0x{:x}", value));
    } else {
        ui.text(format!("{}", value));
    }
}

fn reg_window(ui: &imgui::Ui, hex_values: &mut bool, state: &DebugeeState) {
    let w = ui.window("Regs")
        .position([0.0, 300.0], imgui::Condition::FirstUseEver)
        .size([300.0, 100.0], imgui::Condition::FirstUseEver)
        .begin();
    if w.is_none() {
        return;
    }
    let w = w.unwrap();

    ui.checkbox("Hex", hex_values);

    let col_setup = [ imgui::TableColumnSetup::new("Register"), imgui::TableColumnSetup::new("Value") ];
    let table_token = ui.begin_table_header_with_sizing("##", col_setup, imgui::TableFlags::ROW_BG | imgui::TableFlags::BORDERS | imgui::TableFlags::SCROLL_Y, [ 0.0, 0.0 ], 100.0 );
    if table_token.is_none() {
        return;
    }
    let table_token = table_token.unwrap();

    //ui.table_next_column();

    reg_row(ui, *hex_values, "r15", state.regs.r15);
    reg_row(ui, *hex_values, "r14", state.regs.r14);
    reg_row(ui, *hex_values, "r13", state.regs.r13);
    reg_row(ui, *hex_values, "r12", state.regs.r12);
    reg_row(ui, *hex_values, "rbp", state.regs.rbp);
    reg_row(ui, *hex_values, "rbx", state.regs.rbx);
    reg_row(ui, *hex_values, "r11", state.regs.r11);
    reg_row(ui, *hex_values, "r10", state.regs.r10);
    reg_row(ui, *hex_values, "r9", state.regs.r9);
    reg_row(ui, *hex_values, "r8", state.regs.r8);
    reg_row(ui, *hex_values, "rax", state.regs.rax);
    reg_row(ui, *hex_values, "rcx", state.regs.rcx);
    reg_row(ui, *hex_values, "rdx", state.regs.rdx);
    reg_row(ui, *hex_values, "rsi", state.regs.rsi);
    reg_row(ui, *hex_values, "rdi", state.regs.rdi);
    reg_row(ui, *hex_values, "orig_rax", state.regs.orig_rax);
    reg_row(ui, *hex_values, "rip", state.regs.rip);
    reg_row(ui, *hex_values, "cs", state.regs.cs);
    reg_row(ui, *hex_values, "eflags", state.regs.eflags);
    reg_row(ui, *hex_values, "rsp", state.regs.rsp);
    reg_row(ui, *hex_values, "ss", state.regs.ss);
    reg_row(ui, *hex_values, "fs_base", state.regs.fs_base);
    reg_row(ui, *hex_values, "gs_base", state.regs.gs_base);
    reg_row(ui, *hex_values, "ds", state.regs.ds);
    reg_row(ui, *hex_values, "es", state.regs.es);
    reg_row(ui, *hex_values, "fs", state.regs.fs);
    reg_row(ui, *hex_values, "gs", state.regs.gs);

    table_token.end();
    w.end();
}

fn stack_row(ui: &imgui::Ui, col0: &str, col1: &str) {
    ui.table_next_column();
    ui.text(col0);
    ui.table_next_column();
    ui.text(col1);
}

fn stack_window(ui: &imgui::Ui, pid: Pid, state: &DebugeeState, function_ranges: &Vec<Function>) {
    let w = ui.window("Stack trace")
        .position([0.0, 300.0], imgui::Condition::FirstUseEver)
        .size([300.0, 100.0], imgui::Condition::FirstUseEver)
        .begin();
    if w.is_none() {
        return;
    }
    let w = w.unwrap();

    let red = Vector4{ x: 1.0, y: 0.2, z: 0.2, w: 1.0};
    ui.text_colored(red, "Stack is inverted compared to what you're used to! Top frame is the oldest frame!");

    let col_setup = [ imgui::TableColumnSetup::new("Frame index"), imgui::TableColumnSetup::new("Src") ];
    let table_token = ui.begin_table_header_with_sizing("##", col_setup, imgui::TableFlags::ROW_BG | imgui::TableFlags::BORDERS | imgui::TableFlags::SCROLL_Y, [ 0.0, 0.0 ], 100.0 );
    if table_token.is_none() {
        return;
    }
    let table_token = table_token.unwrap();

    let mut names = vec![""; 32];
    let mut frame_bases = vec![0; 32];

    // TODO: patcher should be split into patcher and data fetcher or smth. And stack unwinding
    // should be done in that new fetcher
    // Also this is fucking horrendous. We need a way to get a line in the program from address.
    let mut frame_base = state.regs.rbp;
    let mut ret_addr = state.regs.rip;
    let mut frame_counter = 0;
    loop {
        if frame_base == 0 {
            break;
        }

        ret_addr -= 0x555555555040 - 0x1040;

        let mut found = false;
        for func in function_ranges.iter() {
            if func.low_pc <= ret_addr && ret_addr <= func.high_pc {
                names[frame_counter] = &func.name;
                frame_bases[frame_counter] = frame_base;
                frame_counter += 1;
                found = true;
            }
        }

        if !found {
            break;
        }

        ret_addr = ptrace::read(pid, (frame_base + 8) as *mut c_void).unwrap() as u64;
        frame_base = ptrace::read(pid, frame_base as *mut c_void).unwrap() as u64;
    }

    for i in (0..frame_counter).rev() {
        stack_row(ui, &format!("Frame #{}", frame_counter - i - 1), &format!("{} at 0x{:x}", names[i], frame_bases[i])); // Well the address is a lie, but whatever
    }

    table_token.end();
    w.end();
}

fn main() {
    let mut system = support::init(file!());

    let mut ctx = DebuggerContext { path_input: "/home/savas/Projects/degrugger/test_code/stack_test.out".to_owned(), session: Err(()), hex_values: true };
    let line_num_str: Vec<String> = (1..100000).map(|x| format!("{: >4}   ", x)).collect();

    system.imgui.io_mut().config_flags |= ConfigFlags::DOCKING_ENABLE;
    let mut first_time = true;
    system.main_loop(move |_, ui| {
        //system.imgui.io_mut().config_flags |= ConfigFlags::DOCKING_ENABLE;

        //let ui = imgui::dock_space::Ui{};
        //ui.dockspace_over_main_viewport();
        unsafe {
            //sys::igDockSpaceOverViewport(
            //    sys::igGetMainViewport(),
            //    //sys::ImGuiDockNodeFlags_PassthruCentralNode as i32,
            //    0 as i32,
            //    null(),
            //);

            let dockspace_flags = sys::ImGuiDockNodeFlags_PassthruCentralNode;
            let mut window_flags = sys::ImGuiWindowFlags_MenuBar | sys::ImGuiWindowFlags_NoDocking;
            let viewport = sys::igGetMainViewport();
            sys::igSetNextWindowPos((*viewport).Pos, sys::ImGuiCond_None.try_into().unwrap(), sys::ImVec2{x: 0.0, y: 0.0});
            sys::igSetNextWindowSize((*viewport).Size, sys::ImGuiCond_None.try_into().unwrap());
            sys::igSetNextWindowViewport((*viewport).ID);
            sys::igPushStyleVar_Float(sys::ImGuiStyleVar_WindowRounding.try_into().unwrap(), 0.0);
            sys::igPushStyleVar_Float(sys::ImGuiStyleVar_WindowBorderSize.try_into().unwrap(), 0.0);
            window_flags |= sys::ImGuiWindowFlags_NoTitleBar | sys::ImGuiWindowFlags_NoCollapse | sys::ImGuiWindowFlags_NoResize | sys::ImGuiWindowFlags_NoMove;
            window_flags |= sys::ImGuiWindowFlags_NoBringToFrontOnFocus | sys::ImGuiWindowFlags_NoNavFocus;
            //if dockspace_flags & ImGuiDockNodeFlags_PassthruCentralNode {
            //    window_flags |= ImGuiWindowFlags_NoBackground;
            //}

            sys::igPushStyleVar_Vec2(sys::ImGuiStyleVar_WindowPadding.try_into().unwrap(), sys::ImVec2{x:0.0, y:0.0});
            let mut t = true;
            sys::igBegin("DockSpace".as_ptr() as *const i8, &mut t as *mut bool, window_flags.try_into().unwrap());
            //sys::igBegin("DockSpace".as_ptr() as *const i8, std::ptr::null::<bool>() as *mut bool, window_flags.try_into().unwrap());
            sys::igPopStyleVar(1);
            sys::igPopStyleVar(2);

            let mut dockspace_id = sys::igGetID_Str("MyDockSpace".as_ptr() as *const i8);
            sys::igDockSpace(dockspace_id, sys::ImVec2{x: 0.0, y: 0.0}, dockspace_flags.try_into().unwrap(), std::ptr::null() as *const sys::ImGuiWindowClass);
            //let mut first_time = true;
            //if first_time {
            if first_time {
                first_time = false;
                sys::igDockBuilderRemoveNode(dockspace_id);
                sys::igDockBuilderAddNode(dockspace_id, (dockspace_flags | sys::ImGuiDockNodeFlags_DockSpace as u32).try_into().unwrap());
                sys::igDockBuilderSetNodeSize(dockspace_id, (*viewport).Size);

                //let dock_id_left = sys::igDockBuilderSplitNode(dockspace_id, sys::ImGuiDir_Left, 0.2, &mut a as *mut u32, &mut dockspace_id as *mut u32);
                //let dock_id_left = sys::igDockBuilderSplitNode(dockspace_id, sys::ImGuiDir_Left, 0.2, &mut a as *mut u32, &mut dockspace_id_copy as *mut u32);
                let mut dock_id_down = sys::igDockBuilderSplitNode(dockspace_id, sys::ImGuiDir_Down, 0.25, std::ptr::null::<u32>() as *mut u32, &mut dockspace_id as *mut u32);
                //sys::igDockBuilderDockWindow("Regs".as_ptr() as *const i8, dock_id_left);

                let mut buf = imgui::UiBuffer::new(16);
                let mut buf2 = imgui::UiBuffer::new(16);
                let mut buf3 = imgui::UiBuffer::new(16);

                buf.scratch_txt("Src code");
                sys::igDockBuilderDockWindow(buf.buffer.as_ptr() as *const i8, dockspace_id);

                buf2.scratch_txt("Regs");
                sys::igDockBuilderDockWindow(buf2.buffer.as_ptr() as *const i8, dock_id_down);

                let left_to_regs = sys::igDockBuilderSplitNode(dock_id_down, sys::ImGuiDir_Right, 0.5, std::ptr::null::<u32>() as *mut u32, &mut dock_id_down as *mut u32);
                buf3.scratch_txt("Stack trace");
                sys::igDockBuilderDockWindow(buf3.buffer.as_ptr() as *const i8, left_to_regs);

                sys::igDockBuilderFinish(dockspace_id);
            }
            sys::igEnd();
        }

        // Poll debugee events now. Don't want 1 frame delays (－‸ლ )
        if let Ok(s) = &mut ctx.session {
            if let Some(r) = &mut s.active_run {
                r.poll_debugee_state(false);

                // TODO: this is some atrocious shit
                if let Some(Err(nix::errno::Errno::EOWNERDEAD)) = r.debugee_event {
                    r.kill();
                    s.active_run = None;
                }
            }
        }

        //let mut t = true;
        //ui.show_metrics_window(&mut t);

        main_menu(ui, &mut ctx, &mut first_time);

        if let Ok(s) = &mut ctx.session {
            let mut maybe_state = &None;

            if let Some(r) = &s.active_run {
                if let Some(state) = &r.debugee_state {
                    reg_window(ui, &mut ctx.hex_values, &state);
                    stack_window(ui, r.debugee_pid, &state, &s.function_ranges);
                }
                maybe_state = &r.debugee_state;
            }

            code_windows(ui, &s.open_files, maybe_state, &line_num_str, &mut s.breakpoints);
            //inlined_stack_window(ui, stopped_state);
            //stack_window(ui, stopped_state);
        }
    });
}
