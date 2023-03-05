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

struct DebuggerContext<'a> {
    path_input: String,
    session: Result<Session<'a>, ()>,
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

    // Line num to text separator
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
                if let Some(bp) = matching_bp {
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

fn main_menu(ui: &imgui::Ui, ctx: &mut DebuggerContext) {
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

    ui.disabled(ctx.session.is_err(), || {
        if ui.arrow_button("##", imgui::Direction::Right) {
            if let Ok(s) = &mut ctx.session {
                s.start_run();
            }
        }
    });

    ui.disabled(ctx.session.is_err() || ctx.session.as_ref().unwrap().active_run.is_none(), || {
        if ui.arrow_button("##", imgui::Direction::Down) {

        }

        if ui.arrow_button("##", imgui::Direction::Up) {

        }
    });

    main_menu_token.end();
}

fn main() {
    let mut system = support::init(file!());

    let mut ctx = DebuggerContext { path_input: "/home/savas/Projects/degrugger/test_code/a.out".to_owned(), session: Err(()) };
    let line_num_str: Vec<String> = (1..100000).map(|x| format!("{: >4}   ", x)).collect();

    system.imgui.io_mut().config_flags |= ConfigFlags::DOCKING_ENABLE;
    system.main_loop(move |_, ui| {
        //system.imgui.io_mut().config_flags |= ConfigFlags::DOCKING_ENABLE;

        //let ui = imgui::dock_space::Ui{};
        //ui.dockspace_over_main_viewport();
        unsafe {
            sys::igDockSpaceOverViewport(
                sys::igGetMainViewport(),
                //sys::ImGuiDockNodeFlags_PassthruCentralNode as i32,
                0 as i32,
                null(),
            );
        }

        let mut t = true;
        ui.show_metrics_window(&mut t);

        main_menu(ui, &mut ctx);

        if let Ok(s) = &mut ctx.session {
            for file in &s.open_files {
                code_window(ui, file, &line_num_str, &mut s.breakpoints);
            }
        }
    });
}
