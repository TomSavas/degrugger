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

mod offline_debug_info;
use crate::offline_debug_info::*;

use std::collections::HashMap;

struct UserInputs {
    cont: bool,
    focus_bp: bool,
}

struct DebuggerContext<'a> {
    path_input: String,
    session: Result<Session<'a>, ()>,

    hex_values: bool,

    user_inputs: UserInputs,
}

//fn text_window(ui: &imgui::Ui, state: &Option<DebugeeState>, line_nums: &Vec<String>, lines: &Vec<String>, breakpoints: &mut Vec<BreakPoint>, debug_info: &HashMap<u64, Arc<SrcFileDebugInfo>>)

#[derive(Debug)]
struct StackNode {
    color: Vector4<f32>, 
    selected: bool,
    subprogram: Subprogram,
    location: Option<BreakableSrcLocation>,
    file_hash: Option<u64>,
    addr: u64,
    //folded: bool,
    //hovered: bool,
}

fn generate_stack(pid: Pid, state: &DebugeeState, debug_info: &ThinOfflineDebugInfo) -> Vec<StackNode> {
    let mut stack = vec![];

    let mut frame_base = state.regs.rbp;
    let mut ret_addr = state.regs.rip - 1;
    let mut frame_counter = 0;
    loop {
        if frame_base == 0 {
            break;
        }

        ret_addr -= 0x555555555040 - 0x1040;
        let mut call_addr = match &debug_info.decompiled_src {
            Some(src) => {
                let mut prev_addr = ret_addr;
                for addr in &src.addresses {
                    if *addr == ret_addr {
                        break;
                    }
                    prev_addr = *addr;
                }
                prev_addr
            },
            None => ret_addr,
        };
        if stack.len() == 0 {
            // First hit has the correct addr
            call_addr = ret_addr;
        }

        let mut found = false;
        let mut subprogram_index = 0;
        for subprogram in &*debug_info.all_subprograms {
            let ret_addr_in_subprogram = subprogram.low_addr <= ret_addr && ret_addr <= subprogram.high_addr;
            let call_addr_in_subprogram = subprogram.low_addr <= call_addr && call_addr <= subprogram.high_addr;

            if ret_addr_in_subprogram && call_addr_in_subprogram {
                let mut bp_location = None;
                let mut file_hash = None;
                'outer: for (hash, src_file_info) in &debug_info.src_file_info {
                    for bp in &src_file_info.breakable_locations {
                        // TODO: this is bad!!!!!
                        if bp.addr == call_addr {
                            bp_location = Some(bp.clone());
                            file_hash = Some(*hash);
                            break 'outer;
                        }
                        if bp.addr == ret_addr {
                            bp_location = Some(bp.clone());
                            file_hash = Some(*hash);
                            break 'outer;
                        }
                    }
                }

                frame_counter += 1;
                found = true;

                let c = match subprogram_index % 6 {
                    0 => Vector4{ x: 0.0, y: 0.0, z: 1.0, w: 1.0 },
                    1 => Vector4{ x: 0.0, y: 1.0, z: 0.0, w: 1.0 },
                    2 => Vector4{ x: 0.0, y: 1.0, z: 1.0, w: 1.0 },
                    3 => Vector4{ x: 1.0, y: 0.0, z: 0.0, w: 1.0 },
                    4 => Vector4{ x: 1.0, y: 0.0, z: 1.0, w: 1.0 },
                    5 => Vector4{ x: 1.0, y: 1.0, z: 0.0, w: 1.0 },
                    _ => Vector4{ x: 0.0, y: 0.0, z: 0.0, w: 0.0 },
                };
                let node = StackNode{ color: c, selected: false, subprogram: subprogram.clone(), location: bp_location, file_hash: file_hash, addr: call_addr };
                //println!("{:?}", node);
                //println!("{ret_addr}, {call_addr}; {:x}, {:x}", ret_addr, call_addr);
                stack.push(node);

            }
            subprogram_index += 1;
        }

        if !found {
            break;
        }

        // TODO: move this. Can crash if the child has died
        ret_addr = ptrace::read(pid, (frame_base + 8) as *mut c_void).unwrap() as u64;
        frame_base = ptrace::read(pid, frame_base as *mut c_void).unwrap() as u64;
    }
    if stack.len() > 0 {
        stack[0].selected = true;
    }

    //println!("");
    //println!("");

    return stack.into_iter().rev().collect();
}

fn inlined_stack_window(ui: &imgui::Ui, state: &DebugeeState, line_num_str: &Vec<String>, breakpoints: &mut Vec<BreakPoint>, debug_info: &ThinOfflineDebugInfo, stack: &Vec<StackNode>, src_files: &HashMap<u64, Arc<SrcFile>>) {
    let w = ui.window("Inline stack").begin();
    if w.is_none() {
        return;
    }
    let w = w.unwrap();

    let char_height = ui.calc_text_size(&" ")[1];
    let char_width = ui.calc_text_size(&" ")[0];

    let draw_list = ui.get_window_draw_list();
    let start_cursor = ui.cursor_screen_pos();
    let content_size = ui.content_region_max();
    let scroll_x = ui.scroll_x();
    let scroll_y = ui.scroll_y();

    let mut start = Vector2{ x: 0.0, y: 0.0 };
    let mut end = Vector2{ x: 0.0, y: 0.0 };
    let mut actual_line_num = 0;
    for node in stack {
        let mut background_color = node.color;
        background_color.w = match node.selected {
            true => 0.3,
            false => 0.1,
        };

        let src_file = match src_files.get(&node.file_hash.unwrap()) {
            Some(src) => src,
            None => continue,
        };
        if src_file.lines.is_none() {
            continue;
        }

        let subprogram = &node.subprogram;
        let end_line = match &node.location {
            Some(l) => l.src_line,
            None => subprogram.end_line,
        };

        for line_num in subprogram.start_line - 1..end_line {
            let line = &src_file.lines.as_ref().unwrap()[line_num];
            let is_bp_line = false;
            if is_bp_line {
                background_color = Vector4{ x: 1.0, y: 0.8, z: 0.0, w: 0.7};
            }

            start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (actual_line_num as f32) * char_height };
            end = Vector2{ x: start.x + content_size[0], y: start.y + char_height };
            draw_list.add_rect(start, end, Vector4{ x: 0.1, y: 0.1, z: 0.1, w: 1.0}).filled(true).build();
            draw_list.add_rect(start, end, background_color).filled(true).build();

            // Line number
            draw_list.add_text(start, ImColor32::WHITE, &line_num_str[line_num]);
            start.x += ui.calc_text_size(&line_num_str[line_num])[0];

            // Code
            draw_list.add_text(start, ImColor32::WHITE, line);

            actual_line_num += 1;
        }

        if end_line != subprogram.end_line + 1 {
            start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (actual_line_num as f32) * char_height };
            end = Vector2{ x: start.x + content_size[0], y: start.y + char_height };
            draw_list.add_rect(start, end, Vector4{ x: 0.1, y: 0.1, z: 0.1, w: 1.0}).filled(true).build();
            let mut darker_background = background_color;
            darker_background.w /= 3.0;
            draw_list.add_rect(start, end, darker_background).filled(true).build();

            draw_list.add_text(start, Vector4{ x: 1.0, y: 0.2, z: 0.2, w: 1.0}, " fold");
            start.x += ui.calc_text_size(&line_num_str[0])[0];

            actual_line_num += 1;
        }
    }
    ui.dummy(Vector2{ x: end.x - start_cursor[0], y: end.y - start_cursor[1] });

    // Code text

    // Line num - text separator
    let c = Vector4{ x: 0.3, y: 0.3, z: 0.3, w: 1.0};
    let start = Vector2{ x: start_cursor[0] + char_width * 5.0, y: start_cursor[1]};
    let e = Vector2{ x: start.x + char_width, y: start.y + char_height * (actual_line_num as f32) };
    draw_list.add_rect(start, e, c).filled(true).build();

    w.end();
    // BPs
    //line_num = 0;
    //for _ in lines {
    //    let start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height };
    //    let end = Vector2{ x: start.x + char_width * 6.0, y: start.y + char_height };

    //    let mut enabled = false;
    //    if let Some(addr) = file.line_to_addr.get(&(line_num + 1)) {
    //        let mut matching_bp: Option<&mut BreakPoint> = None;
    //        for bp in breakpoints.iter_mut() {
    //            if bp.point.addr == *addr {
    //                enabled = bp.point.enabled;
    //                matching_bp = Some(bp);
    //                break;
    //            }
    //        }

    //        if ui.is_mouse_hovering_rect(start, end) && ui.is_mouse_clicked(imgui::MouseButton::Left) {
    //            if let Some(ref mut bp) = matching_bp {
    //                enabled = false;
    //                bp.point.enabled = enabled;
    //            } else {
    //                enabled = true;
    //                breakpoints.push(BreakPoint::new(Point::new(*addr, (line_num + 1) as u64)));

    //                let max_bp_count = breakpoints.len() - 1;
    //                matching_bp = Some(&mut breakpoints[max_bp_count]);
    //            }
    //        }
    //    }

    //    if enabled {
    //        let r = Vector4{ x: 1.0, y: 0.2, z: 0.2, w: 1.0};
    //        let c = Vector2{ x: start.x + char_width * 5.5, y: start.y + char_height * 0.5 };
    //        draw_list.add_circle(c, char_width / 2.0, r).filled(true).build();
    //    }
    //    line_num += 1;
    //}
}

fn disassembly_window(ui: &imgui::Ui, inputs: &UserInputs, state: &Option<DebugeeState>, line_num_str: &Vec<String>, breakpoints: &mut Vec<BreakPoint>, debug_info: &ThinOfflineDebugInfo)
{
    let w = ui.window("Disassembly").begin();
    if w.is_none() {
        return;
    }
    let w = w.unwrap();
    if debug_info.decompiled_src.is_none() {
        w.end();
        return;
    }
    let decompiled_src = &debug_info.decompiled_src.as_ref().unwrap();
    let lines = &decompiled_src.decompiled_src;

    let char_height = ui.calc_text_size(&" ")[1];
    let char_width = ui.calc_text_size(&" ")[0];


    let draw_list = ui.get_window_draw_list();
    let start_cursor = ui.cursor_screen_pos();
    let content_size = ui.content_region_max();

    let mut bp_addr = match state {
        Some(s) => s.regs.rip - (0x555555555040 - 0x1040) - 1,
        None => 0,
    };

    let scroll_max_y = ui.scroll_max_y();
    if inputs.focus_bp {
        for i in 0..lines.len() {
            let addr = decompiled_src.addresses[i];
            if bp_addr == addr {
                let perc = i as f32 / lines.len() as f32;
                ui.set_scroll_y(perc * scroll_max_y);
            }
        }
    }
    let scroll_x = ui.scroll_x();
    let scroll_y = ui.scroll_y();

    // Line background

    let starting_line = ((scroll_y as f32 - char_height + 1.0) / char_height) as usize;
    let adjusted_content_size = Vector2{x: content_size[0], y:content_size[1] + scroll_y};
    let visible_lines = (adjusted_content_size.y as f32 / char_height) as usize + 1;
    let ending_line = std::cmp::min(lines.len(), starting_line + visible_lines);

    //println!("{starting_line} {visible_lines} {ending_line}");

    let mut end = Vector2{ x: 0.0, y: 0.0 };
    for (i, line) in lines[starting_line..ending_line].iter().enumerate() {
        let addr = decompiled_src.addresses[i + starting_line];
        let line_num = i + starting_line;
        let red = Vector4{ x: 0.1, y: 0.1, z: 0.1, w: 1.0};
        let green = Vector4{ x: 0.2, y: 0.2, z: 0.2, w: 1.0};

        // Function coloring
        let mut subprogram_index = 0;
        let mut found_subprogram = false;
        let mut is_bp_func = false;
        for subprogram in &*debug_info.all_subprograms {
            if subprogram.low_addr <= addr && addr <= subprogram.high_addr {
                found_subprogram = true;
                is_bp_func = subprogram.low_addr <= bp_addr && bp_addr <= subprogram.high_addr;
                break;
            }
            subprogram_index += 1;
        }
    
        let mut background_color = red;
        //let mut background_color = if line_num % 2 == 0 { red } else { green };
        let is_bp_line = bp_addr == addr;
        if is_bp_line {
            background_color = Vector4{ x: 1.0, y: 0.8, z: 0.0, w: 0.7};
        }

        let mut start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height };
        end = Vector2{ x: start.x + content_size[0], y: start.y + char_height };
        draw_list.add_rect(start, end, background_color).filled(true).build();

        if found_subprogram && !is_bp_line {
            let alpha = match is_bp_func {
                true => 0.3,
                false => 0.1,
            };
            let c = match subprogram_index % 6 {
                0 => Vector4{ x: 0.0, y: 0.0, z: 1.0, w: alpha },
                1 => Vector4{ x: 0.0, y: 1.0, z: 0.0, w: alpha },
                2 => Vector4{ x: 0.0, y: 1.0, z: 1.0, w: alpha },
                3 => Vector4{ x: 1.0, y: 0.0, z: 0.0, w: alpha },
                4 => Vector4{ x: 1.0, y: 0.0, z: 1.0, w: alpha },
                5 => Vector4{ x: 1.0, y: 1.0, z: 0.0, w: alpha },
                _ => Vector4{ x: 0.0, y: 0.0, z: 0.0, w: 0.0 },
            };
            draw_list.add_rect(start, end, c).filled(true).build();
        }

        //BP-able locations
        //let maybe_debug_info = &debug_info.get(&file.simple_hash());
        //if let Some(debug_info) = maybe_debug_info {
        //    for breakable_location in &debug_info.breakable_locations {
        //        if line_num + 1 == breakable_location.src_line {
        //            //println!("At line: {}, col: {}, addr: {}", breakable_location.src_line, breakable_location.src_col, breakable_location.addr.0);
        //            let c = Vector4{ x: 1.0, y: 0.0, z: 0.0, w: 0.3};
        //            let s = Vector2{ x: start.x + char_width * (6.0 + breakable_location.src_col as f32), y: start.y};
        //            let e = Vector2{ x: s.x + char_width * 1.0, y: end.y};
        //            draw_list.add_rect(s, e, c).filled(true).build();
        //        }
        //    }
        //}

        // Line number
        draw_list.add_text(start, ImColor32::WHITE, &line_num_str[line_num]);
        start.x += ui.calc_text_size(&line_num_str[line_num])[0];

        // Code
        draw_list.add_text(start, ImColor32::WHITE, line);
    }
    ui.dummy(Vector2{ x: end.x - start_cursor[0], y: char_height * lines.len() as f32 });

    // Code text

    // Line num - text separator
    let c = Vector4{ x: 0.3, y: 0.3, z: 0.3, w: 1.0};
    let start = Vector2{ x: start_cursor[0] + char_width * 5.0, y: start_cursor[1] - scroll_y};
    let e = Vector2{ x: start.x + char_width, y: start.y + char_height * (visible_lines as f32) };
    draw_list.add_rect(start, e, c).filled(true).build();

    // BPs
    //line_num = 0;
    //for _ in lines {
    //    let start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height - scroll_y };
    //    let end = Vector2{ x: start.x + char_width * 6.0, y: start.y + char_height };

    //    let mut enabled = false;
    //    if let Some(addr) = file.line_to_addr.get(&(line_num + 1)) {
    //        let mut matching_bp: Option<&mut BreakPoint> = None;
    //        for bp in breakpoints.iter_mut() {
    //            if bp.point.addr == *addr {
    //                enabled = bp.point.enabled;
    //                matching_bp = Some(bp);
    //                break;
    //            }
    //        }

    //        if ui.is_mouse_hovering_rect(start, end) && ui.is_mouse_clicked(imgui::MouseButton::Left) {
    //            if let Some(ref mut bp) = matching_bp {
    //                enabled = false;
    //                bp.point.enabled = enabled;
    //            } else {
    //                enabled = true;
    //                breakpoints.push(BreakPoint::new(Point::new(*addr, (line_num + 1) as u64)));

    //                let max_bp_count = breakpoints.len() - 1;
    //                matching_bp = Some(&mut breakpoints[max_bp_count]);
    //            }
    //        }
    //    }

    //    if enabled {
    //        let r = Vector4{ x: 1.0, y: 0.2, z: 0.2, w: 1.0};
    //        let c = Vector2{ x: start.x + char_width * 5.5, y: start.y + char_height * 0.5 };
    //        draw_list.add_circle(c, char_width / 2.0, r).filled(true).build();
    //    }
    //    line_num += 1;
    //}
    w.end();
}

fn code_windows(ui: &imgui::Ui, user_inputs: &UserInputs, files: &Vec<SrcFile>, state: &Option<DebugeeState>, line_num_str: &Vec<String>, breakpoints: &mut Vec<BreakPoint>, debug_info: &ThinOfflineDebugInfo) {
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
            code_windoww(ui, user_inputs, file, state, &line_num_str, breakpoints, debug_info);
            tab_item.end();
            //break;
        }
    }

    t.end();
    w.end();
}

fn code_windoww(ui: &imgui::Ui, inputs: &UserInputs, file: &SrcFile, state: &Option<DebugeeState>, line_num_str: &Vec<String>, breakpoints: &mut Vec<BreakPoint>, debug_info: &ThinOfflineDebugInfo) {
    if file.lines.is_none() {
        return;
    }
    let hash = file.simple_hash();
    let lines = file.lines.as_ref().unwrap();

    let char_height = ui.calc_text_size(&" ")[1];
    let char_width = ui.calc_text_size(&" ")[0];

    let draw_list = ui.get_window_draw_list();
    let start_cursor = ui.cursor_screen_pos();
    let content_size = ui.content_region_max();

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

    let scroll_max_y = ui.scroll_max_y();
    if inputs.focus_bp {
        let perc = bp_line as f32 / lines.len() as f32;
        ui.set_scroll_y(perc * scroll_max_y);
    }
    let scroll_x = ui.scroll_x();
    let scroll_y = ui.scroll_y();
    //if let Some(src_debug_info) = debug_info.src_file_info.get(&hash) {
    //    for subprogram in &*src_debug_info.subprograms {
    //        println!("subprogram {} {} - {}; 0x{:x} - 0x{:x}", subprogram.name, subprogram.start_line, subprogram.end_line, subprogram.low_addr, subprogram.high_addr);
    //    }
    //}

    let mut start = Vector2{ x: 0.0, y: 0.0 };
    let mut end = Vector2{ x: 0.0, y: 0.0 };
    let mut line_num = 0;
    // Line background
    for line in lines {
        let red = Vector4{ x: 0.1, y: 0.1, z: 0.1, w: 1.0};
        let green = Vector4{ x: 0.2, y: 0.2, z: 0.2, w: 1.0};

        // Function coloring
        let mut subprogram_index = 0;
        let mut found_subprogram = false;
        let hash = file.simple_hash();
        let mut is_bp_func = false;
        if let Some(src_debug_info) = debug_info.src_file_info.get(&hash) {
            for subprogram in &*src_debug_info.subprograms {
                //println!("Line {} subprogram {} {} - {}", line_num+1, subprogram.name, subprogram.start_line, subprogram.end_line);
                if hash == subprogram.src_file_hash && subprogram.start_line <= (line_num + 1) && (line_num + 1) <= subprogram.end_line {
                    is_bp_func = subprogram.start_line <= bp_line && bp_line <= subprogram.end_line;
                    found_subprogram = true;
                    break;
                }
                subprogram_index += 1;
            }
        }

        //let mut background_color = if line_num % 2 == 0 { red } else { green };
        let mut background_color = red;
        let is_bp_line = bp_line == (line_num + 1);
        if is_bp_line {
            background_color = Vector4{ x: 1.0, y: 0.8, z: 0.0, w: 0.7};
        }

        start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height };
        end = Vector2{ x: start.x + content_size[0], y: start.y + char_height };
        draw_list.add_rect(start, end, background_color).filled(true).build();

        if found_subprogram && !is_bp_line {
            let alpha = match is_bp_func {
                true => 0.3,
                false => 0.1,
            };
            let c = match subprogram_index % 6 {
                0 => Vector4{ x: 0.0, y: 0.0, z: 1.0, w: alpha },
                1 => Vector4{ x: 0.0, y: 1.0, z: 0.0, w: alpha },
                2 => Vector4{ x: 0.0, y: 1.0, z: 1.0, w: alpha },
                3 => Vector4{ x: 1.0, y: 0.0, z: 0.0, w: alpha },
                4 => Vector4{ x: 1.0, y: 0.0, z: 1.0, w: alpha },
                5 => Vector4{ x: 1.0, y: 1.0, z: 0.0, w: alpha },
                _ => Vector4{ x: 0.0, y: 0.0, z: 0.0, w: 0.0 },
            };
            draw_list.add_rect(start, end, c).filled(true).build();
        }

        //BP-able locations
        //let maybe_debug_info = &debug_info.src_file_info.get(&file.simple_hash());
        //if let Some(debug_info) = maybe_debug_info {
        //    for breakable_location in &debug_info.breakable_locations {
        //        if line_num + 1 == breakable_location.src_line {
        //            //println!("At line: {}, col: {}, addr: {}", breakable_location.src_line, breakable_location.src_col, breakable_location.addr.0);
        //            let c = Vector4{ x: 1.0, y: 0.0, z: 0.0, w: 0.3};
        //            let s = Vector2{ x: start.x + char_width * (6.0 + breakable_location.src_col as f32), y: start.y};
        //            let e = Vector2{ x: s.x + char_width * 1.0, y: end.y};
        //            draw_list.add_rect(s, e, c).filled(true).build();
        //        }
        //    }
        //}

        // Line number
        draw_list.add_text(start, ImColor32::WHITE, &line_num_str[line_num]);
        start.x += ui.calc_text_size(&line_num_str[line_num])[0];

        // Code
        draw_list.add_text(start, ImColor32::WHITE, line);

        //ui.dummy(Vector2{ x: end.x - start.x, y: end.y - start.y });

        line_num += 1;
    }
    ui.dummy(Vector2{ x: end.x - start_cursor[0], y: end.y - start_cursor[1] });
    //ui.dummy(Vector2{ x: end.x - start.x, y: char_height * lines.len() as f32 * 0.5 });

    // Code text

    // Line num - text separator
    let c = Vector4{ x: 0.3, y: 0.3, z: 0.3, w: 1.0};
    let start = Vector2{ x: start_cursor[0] + char_width * 5.0, y: start_cursor[1]};
    let e = Vector2{ x: start.x + char_width, y: start.y + char_height * (line_num as f32) };
    draw_list.add_rect(start, e, c).filled(true).build();

    // BPs
    line_num = 0;
    for _ in lines {
        let start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height };
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

        let mut start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height };
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
        let start = Vector2{ x: start_cursor[0] + scroll_x, y: start_cursor[1] + (line_num as f32) * char_height };
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
        if ui.button("Continue") || ctx.user_inputs.cont {
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

fn stack_row(ui: &imgui::Ui, col0: &str, col1: &str, col2: &str) {
    ui.text(col0);
    ui.table_next_column();
    ui.text(col1);
    ui.table_next_column();
    ui.text(col2);
    ui.table_next_column();
}

fn stack_window(ui: &imgui::Ui, pid: Pid, state: &DebugeeState, function_ranges: &Vec<Function>, debug_info: &ThinOfflineDebugInfo, stack: &Vec<StackNode>) {
    let w = ui.window("Stack trace")
        .position([0.0, 300.0], imgui::Condition::FirstUseEver)
        .size([300.0, 100.0], imgui::Condition::FirstUseEver)
        .begin();
    if w.is_none() {
        return;
    }
    let w = w.unwrap();

    if debug_info.decompiled_src.is_none() {
        w.end();
        return;
    }
    let decompiled_src = &debug_info.decompiled_src.as_ref().unwrap();

    let red = Vector4{ x: 1.0, y: 0.2, z: 0.2, w: 1.0};
    ui.text_colored(red, "Stack is inverted compared to what you're used to! Top frame is the oldest frame!");

    let col_setup = [ imgui::TableColumnSetup::new("Frame index"), imgui::TableColumnSetup::new("Function"), imgui::TableColumnSetup::new("Address") ];
    let table_token = ui.begin_table_header_with_sizing("##", col_setup, imgui::TableFlags::ROW_BG | imgui::TableFlags::BORDERS | imgui::TableFlags::SCROLL_Y, [ 0.0, 0.0 ], 100.0 );
    if table_token.is_none() {
        return;
    }
    let table_token = table_token.unwrap();
    ui.table_next_column();

    for (i, node) in stack.iter().enumerate() {
        //stack_row(ui, &format!("Frame #{}", frame_counter - i - 1), &format!("{} at 0x{:x}", names[i], ret_addresses[i]));
        let function = match &node.location {
            Some(l) => format!("{}:{}:{}", &node.subprogram.name, l.src_line, l.src_col),
            None => node.subprogram.name.clone(),
        };
        let mut c = node.color;
        c.w = match node.selected {
            true => 0.3,
            false => 0.1
        };

        // Manually alpha blend
        let bg = Vector4{ x: 0.1, y: 0.1, z: 0.1, w: 1.0 };
        let blended_alpha = c.w + bg.w * (1.0 - c.w);
        let blended_c = Vector4{ 
            x: (c.x * c.w + bg.x * bg.w * (1.0 - c.w)) / blended_alpha,
            y: (c.y * c.w + bg.y * bg.w * (1.0 - c.w)) / blended_alpha,
            z: (c.z * c.w + bg.z * bg.w * (1.0 - c.w)) / blended_alpha,
            w: blended_alpha,
        };

        ui.table_set_bg_color(imgui::TableBgTarget::ROW_BG0, blended_c);
        stack_row(ui, &format!("Frame #{}", i), &function, &format!("0x{:x}", node.addr));
    }

    table_token.end();
    w.end();
}

use std::sync::Arc;
use std::path::PathBuf;

fn main() {
    //let a = OfflineDebugInfo::new();
    //let a = a.unwrap();
    //let f = SrcFile::new(PathBuf::from("/home/savas/Projects/degrugger/test_code/stack_test.c"), false).unwrap();
    //a.debug_info_request_sender.send(Arc::new(f));

    let mut system = support::init(file!());

    let mut ctx = DebuggerContext { path_input: "/home/savas/Projects/degrugger/test_code/stack_test.out".to_owned(), session: Err(()), hex_values: true, user_inputs: UserInputs{ cont: false, focus_bp: false } };
    let line_num_str: Vec<String> = (1..1000000).map(|x| format!("{: >4}   ", x)).collect();

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

            //let style = sys::igGetStyle();
            //sys::ImGuiStyle_ScaleAllSizes(style, 0.1);

            (*sys::igGetIO()).FontGlobalScale = 0.66;

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

                let mut dock_id_right = sys::igDockBuilderSplitNode(dockspace_id, sys::ImGuiDir_Right, 0.6, std::ptr::null::<u32>() as *mut u32, &mut dockspace_id as *mut u32);
                //sys::igDockBuilderDockWindow("Regs".as_ptr() as *const i8, dock_id_left);

                let mut dock_id_right_right = sys::igDockBuilderSplitNode(dock_id_right, sys::ImGuiDir_Right, 0.5, std::ptr::null::<u32>() as *mut u32, &mut dock_id_right as *mut u32);

                let mut buf = imgui::UiBuffer::new(16);
                let mut buf2 = imgui::UiBuffer::new(16);
                let mut buf3 = imgui::UiBuffer::new(16);
                let mut buf4 = imgui::UiBuffer::new(16);
                let mut buf5 = imgui::UiBuffer::new(16);

                buf.scratch_txt("Src code");
                sys::igDockBuilderDockWindow(buf.buffer.as_ptr() as *const i8, dockspace_id);

                buf5.scratch_txt("Inline stack");
                sys::igDockBuilderDockWindow(buf5.buffer.as_ptr() as *const i8, dock_id_right);

                buf4.scratch_txt("Disassembly");
                sys::igDockBuilderDockWindow(buf4.buffer.as_ptr() as *const i8, dock_id_right_right);

                buf2.scratch_txt("Regs");
                sys::igDockBuilderDockWindow(buf2.buffer.as_ptr() as *const i8, dock_id_down);

                let left_to_regs = sys::igDockBuilderSplitNode(dock_id_down, sys::ImGuiDir_Right, 0.5, std::ptr::null::<u32>() as *mut u32, &mut dock_id_down as *mut u32);
                buf3.scratch_txt("Stack trace");
                sys::igDockBuilderDockWindow(buf3.buffer.as_ptr() as *const i8, left_to_regs);

                sys::igDockBuilderFinish(dockspace_id);
            }
            sys::igEnd();
        }
        
        let c_pressed = ui.is_key_pressed_no_repeat(imgui::Key::C);
        ctx.user_inputs = UserInputs {
            cont: c_pressed,
            focus_bp: c_pressed || ui.is_key_pressed_no_repeat(imgui::Key::Period),
        };

        // Poll debugee events now. Don't want 1 frame delays (－‸ლ )
        if let Ok(s) = &mut ctx.session {
            s.sync_workers();
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
                    let stack = generate_stack(r.debugee_pid, &state, &s.debug_info.debug_infoo);
                    stack_window(ui, r.debugee_pid, &state, &s.function_ranges, &s.debug_info.debug_infoo, &stack);
                    //stack_window(ui, r.debugee_pid, &state, &s.function_ranges, &s.debug_info.debug_infoo);

                    inlined_stack_window(ui, &state, &line_num_str, &mut s.breakpoints, &s.debug_info.debug_infoo, &stack, &s.debug_info.src_files);
                maybe_state = &r.debugee_state;
                }
            }

            //code_windows(ui, &s.open_files, maybe_state, &line_num_str, &mut s.breakpoints);
            code_windows(ui, &ctx.user_inputs, &s.open_files, maybe_state, &line_num_str, &mut s.breakpoints, &s.debug_info.debug_infoo);
            disassembly_window(ui, &ctx.user_inputs, maybe_state, &line_num_str, &mut s.breakpoints, &s.debug_info.debug_infoo);
            //inlined_stack_window(ui, stopped_state);
            //stack_window(ui, stopped_state);
        }
    });
}
